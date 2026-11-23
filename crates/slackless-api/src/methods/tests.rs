use super::*;
use crate::types::ApiResponse;

/// A captured `activity.feed` answer (personal data replaced): one row per modeled type,
/// an unknown type, and a thread entry without its bundle.
const FEED: &str = r#"{
    "ok": true,
    "items": [
        {
            "is_unread": true,
            "is_bot": false,
            "is_archived": false,
            "version": "v2",
            "feed_ts": "1784892914.789649",
            "key": "thread_v2-C0ENG-1784891984.803759",
            "item": {
                "type": "thread_v2",
                "bundle_info": {
                    "unread_count": 3,
                    "payload": {
                        "thread_entry": {
                            "channel_id": "C0ENG",
                            "thread_ts": "1784891984.803759",
                            "latest_ts": "1784892900.000100",
                            "unread_msg_count": 2
                        }
                    }
                }
            }
        },
        {
            "is_unread": false,
            "is_bot": false,
            "feed_ts": "1784891000.000200",
            "key": "at_user-C0ENG-1784891000.000200",
            "item": {
                "type": "at_user",
                "message": {
                    "ts": "1784891000.000200",
                    "channel": "C0ENG",
                    "author_user_id": "U0AUTHOR",
                    "thread_ts": "1784891984.803759",
                    "is_broadcast": false
                }
            }
        },
        {
            "is_unread": false,
            "feed_ts": "1784890000.000300",
            "key": "at_channel-C0ENG-1784890000.000300",
            "item": {
                "type": "at_channel",
                "message": {
                    "ts": "1784890000.000300",
                    "channel": "C0ENG",
                    "author_user_id": "U0AUTHOR",
                    "is_broadcast": true
                }
            }
        },
        {
            "is_unread": true,
            "feed_ts": "1784889000.000400",
            "key": "message_reaction-C0ENG-1784889000.000400",
            "item": {
                "type": "message_reaction",
                "message": {
                    "ts": "1784800000.000400",
                    "channel": "C0ENG",
                    "author_user_id": "U0ME"
                },
                "reaction": { "user": "U0REACTOR", "name": "eyes" }
            }
        },
        {
            "is_unread": true,
            "feed_ts": "1784888000.000500",
            "key": "dm-D0PEER",
            "item": {
                "type": "dm",
                "bundle_info": {
                    "unread_count": 2,
                    "payload": {
                        "dm_entry": {
                            "latest_message": {
                                "ts": "1784888000.000500",
                                "channel": "D0PEER"
                            }
                        }
                    }
                }
            }
        },
        {
            "is_bot": true,
            "is_unread": false,
            "feed_ts": "1784887000.000600",
            "key": "bot_dm_bundle-D0BOT",
            "item": {
                "type": "bot_dm_bundle",
                "bundle_info": {
                    "unread_count": 0,
                    "payload": {
                        "message": {
                            "ts": "1784887000.000600",
                            "channel": "D0BOT"
                        }
                    }
                }
            }
        },
        {
            "is_unread": false,
            "feed_ts": "1784886000.000700",
            "key": "saved_reminder-X0-1784886000.000700",
            "item": {
                "type": "saved_reminder",
                "saved_reminder_payload": { "item_type": "message" }
            }
        }
    ],
    "response_metadata": { "next_cursor": "abc" }
}"#;

#[test]
fn parses_activity_feed_and_skips_unknown_types() {
    let value: Value = serde_json::from_str(FEED).unwrap();
    let resp: ApiResponse<ActivityFeedWire> = ApiResponse::from_value(value).expect("feed parses");
    let next_cursor = resp.next_cursor().map(str::to_owned);
    let feed = ActivityFeed {
        items: resp.data.items.into_iter().filter_map(row).collect(),
        next_cursor,
    };

    assert_eq!(feed.next_cursor.as_deref(), Some("abc"));
    let items = feed.items;
    assert_eq!(items.len(), 6, "the saved_reminder row is skipped");

    let thread = &items[0];
    assert_eq!(thread.kind, ActivityKind::Thread);
    assert!(thread.unread);
    assert_eq!(thread.channel.as_str(), "C0ENG");
    assert_eq!(
        thread.thread_ts.as_ref().map(|ts| ts.as_str()),
        Some("1784891984.803759")
    );
    assert_eq!(thread.ts.as_str(), "1784892900.000100");
    assert_eq!(
        thread.count, 3,
        "the bundle's unread count wins over the entry's"
    );
    assert!(thread.message.is_none(), "thread rows hydrate separately");

    let mention = &items[1];
    assert_eq!(mention.kind, ActivityKind::Mention);
    assert_eq!(mention.actor.as_ref().map(|u| u.as_str()), Some("U0AUTHOR"));
    assert_eq!(
        mention.thread_ts.as_ref().map(|ts| ts.as_str()),
        Some("1784891984.803759")
    );

    // The at_channel alias parses into the same mention shape.
    assert_eq!(items[2].kind, ActivityKind::Mention);
    assert!(items[2].thread_ts.is_none());

    let reaction = &items[3];
    assert_eq!(reaction.kind, ActivityKind::Reaction);
    assert_eq!(
        reaction.actor.as_ref().map(|u| u.as_str()),
        Some("U0REACTOR")
    );
    let reaction = reaction.reaction.as_ref().unwrap();
    assert_eq!(
        (
            reaction.name.as_str(),
            reaction.user.as_ref().map(|u| u.as_str())
        ),
        ("eyes", Some("U0REACTOR"))
    );

    let dm = &items[4];
    assert_eq!(dm.kind, ActivityKind::Dm);
    assert_eq!(dm.channel.as_str(), "D0PEER", "the ref carries the channel");
    assert_eq!(dm.ts.as_str(), "1784888000.000500");
    assert_eq!(dm.count, 2, "the bundle's unread count becomes the pill");
    assert!(dm.message.is_none(), "a bare ref hydrates separately");

    let app = &items[5];
    assert_eq!(app.kind, ActivityKind::App);
    assert_eq!(app.channel.as_str(), "D0BOT");
    assert!(app.message.is_none());
    assert_eq!(app.count, 0);
}

#[test]
fn team_info_wire_parses_the_connected_org_record() {
    let value: Value = serde_json::from_str(
        r#"{
            "ok": true,
            "team": {
                "id": "T0KLARNA",
                "name": "Klarna",
                "domain": "klarna",
                "email_domain": "klarna.com",
                "icon": {
                    "image_default": true,
                    "image_34": "https://a.slack-edge.com/img/avatars-teams/ava_34.png",
                    "image_68": "https://a.slack-edge.com/img/avatars-teams/ava_68.png"
                },
                "avatar_base_url": "https://ca-slack-atl.com/"
            }
        }"#,
    )
    .unwrap();
    let resp: ApiResponse<TeamWire> = ApiResponse::from_value(value).expect("team parses");
    let team = resp.data.team;
    assert_eq!(team.id.as_str(), "T0KLARNA");
    assert_eq!(team.name, "Klarna");
    assert_eq!(team.domain, "klarna");
    assert_eq!(
        team.icon.as_ref().and_then(|icon| icon.get("image_34")),
        Some(&Value::String(
            "https://a.slack-edge.com/img/avatars-teams/ava_34.png".into()
        )),
        "the icon object survives verbatim for the UI to pick a URL from"
    );
    assert!(team.extra.contains_key("avatar_base_url"));
}

#[test]
fn mark_read_entry_keeps_the_keys_type_prefix() {
    let value: Value = serde_json::from_str(FEED).unwrap();
    let resp: ApiResponse<ActivityFeedWire> = ApiResponse::from_value(value).expect("feed parses");
    let next_cursor = resp.next_cursor().map(str::to_owned);
    let items: Vec<ActivityItem> = resp.data.items.into_iter().filter_map(row).collect();
    assert_eq!(next_cursor.as_deref(), Some("abc"));
    let entries: Vec<ActivityEntry> = items.iter().map(ActivityItem::entry).collect();
    assert_eq!(entries[0].kind, "thread_v2");
    assert_eq!(entries[0].key, "thread_v2-C0ENG-1784891984.803759");
    assert_eq!(entries[0].feed_ts.as_str(), "1784892914.789649");
    // An at_channel mention keeps its own type, not at_user.
    assert_eq!(entries[2].kind, "at_channel");
    assert_eq!(entries[5].kind, "bot_dm_bundle");
    // The wire form is the JSON array Slack expects.
    assert_eq!(
        serde_json::to_string(&entries[0]).unwrap(),
        "{\"type\":\"thread_v2\",\"key\":\"thread_v2-C0ENG-1784891984.803759\",\"feed_ts\":\"1784892914.789649\"}"
    );
}

#[test]
fn messages_list_bodies_parse_by_channel() {
    let wire: MessagesListWire = serde_json::from_str(
        r#"{ "messages_data": {
            "C0ENG": { "messages": [
                { "ts": "1784891000.000200", "user": "U0AUTHOR", "text": "hey <@U0ME>" }
            ] },
            "D0PEER": { "messages": [] }
        } }"#,
    )
    .unwrap();
    let bodies: HashMap<ChannelId, Vec<Message>> = wire
        .messages_data
        .into_iter()
        .map(|(channel, entry)| (ChannelId::from(channel.as_str()), entry.messages))
        .collect();
    let eng = &bodies[&ChannelId::from("C0ENG")];
    assert_eq!((eng.len(), eng[0].text.as_str()), (1, "hey <@U0ME>"));
    assert!(bodies[&ChannelId::from("D0PEER")].is_empty());
}

#[test]
fn client_counts_parses_dm_count_and_clears_cleanly() {
    let mut counts: ClientCounts = serde_json::from_str(
        r#"{
            "ims": [{
                "id": "D0PEER",
                "last_read": "1700000100.000100",
                "latest": "1700000200.000100",
                "dm_count": 3
            }]
        }"#,
    )
    .unwrap();
    let dm = &mut counts.ims[0];
    assert_eq!(dm.dm_count, 3);
    assert_eq!(dm.effective_mentions(), 3);
    assert!(dm.is_unread());
    dm.clear_unreads();
    assert!(!dm.is_unread());
    assert_eq!(dm.effective_mentions(), 0);
}

#[test]
fn rooms_join_wire_parses_chime_free_willy_credentials() {
    let wire: RoomsJoinWire = serde_json::from_str(
        r#"{
            "call": {
                "call_id": "R0123456789",
                "free_willy": {
                    "attendee": {
                        "AttendeeId": "att-1",
                        "ExternalUserId": "T1-R0123456789-U1",
                        "JoinToken": "tok-abc",
                        "Capabilities": { "Audio": "SendReceive", "Video": "SendReceive", "Content": "SendReceive" }
                    },
                    "meeting": {
                        "MeetingId": "mtg-1",
                        "ExternalMeetingId": "T1-R0123456789-U1",
                        "MediaRegion": "eu-central-1",
                        "MediaPlacement": {
                            "SignalingUrl": "wss://signal.chime.aws/control/mtg-1",
                            "AudioHostUrl": "host.chime.aws:3478",
                            "TurnControlUrl": "https://turn.chime.aws/v2/turn_sessions"
                        }
                    }
                },
                "huddle": {
                    "id": "R0123456789",
                    "channels": ["C1"],
                    "has_ended": false,
                    "media_backend_type": "free_willy",
                    "participants": ["U1"],
                    "thread_root_ts": "1700000000.000100",
                    "huddle_link": "https://app.slack.com/huddle/T1/C1"
                },
                "canvas": { "root_thread_ts": "1700000000.000100", "thread_channel_id": "C1" }
            }
        }"#,
    )
    .unwrap();
    let call = wire.call.unwrap();
    assert_eq!(call.call_id, "R0123456789");
    let chime = call.free_willy.unwrap();
    assert_eq!(chime.attendee.join_token, "tok-abc");
    assert_eq!(
        chime.meeting.media_placement.signaling_url,
        "wss://signal.chime.aws/control/mtg-1"
    );
    assert!(call.huddle.is_active());
    assert_eq!(
        call.canvas.unwrap().thread_channel_id,
        Some(ChannelId::from("C1"))
    );
}

/// A one-off probe of the live `activity.feed` answer, printed as raw JSON so the
/// parser is built against the real shape (values carry personal data; the shipped
/// fixture replaces them). Run manually with the signed-in session in the app's store;
/// never in CI:
///
/// ```text
/// CARGO_BUILD_JOBS=4 direnv exec . cargo test -p slackless-api activity_feed_live \
///     -- --ignored --nocapture
/// ```
#[test]
#[ignore = "probes the live workspace; run manually (see the test docs)"]
fn activity_feed_live() {
    use std::process::Command;

    let sessions: Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap())
                .join("Library/Application Support/slackless/sessions.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let team = sessions[0]["team_id"].as_str().unwrap().to_owned();
    let url = sessions[0]["workspace_url"].as_str().unwrap().to_owned();
    let secret = Command::new("/usr/bin/security")
        .args([
            "find-generic-password",
            "-s",
            "slackless",
            "-a",
            &team,
            "-w",
        ])
        .output()
        .unwrap();
    let secret: Value = serde_json::from_slice(secret.stdout.trim_ascii_end()).unwrap();
    let host = url::Url::parse(&url)
        .unwrap()
        .host_str()
        .unwrap()
        .to_owned();
    let domain = host.strip_suffix(".slack.com").unwrap_or(&host);
    let api = SlackApi::for_workspace(
        domain,
        secret["xoxc"].as_str().unwrap(),
        secret["d"].as_str().unwrap(),
    )
    .unwrap();

    // Param bisection: which type lists Slack accepts.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let params: Vec<(&'static str, String)> = vec![
        ("types", ACTIVITY_TYPES.to_owned()),
        ("mode", "chrono_v1".to_owned()),
        ("limit", "50".to_owned()),
        ("is_activity_inbox", "true".to_owned()),
    ];
    let resp = rt
        .block_on(api.call::<Value>(method::ACTIVITY_FEED, params))
        .unwrap();
    println!("{}", serde_json::to_string_pretty(&resp.data).unwrap());

    // messages.list hydration of the first few rows.
    let feed = rt.block_on(api.activity_feed(10)).unwrap();
    let ids: Vec<(ChannelId, Vec<Ts>)> = feed
        .items
        .iter()
        .take(6)
        .map(|item| (item.channel.clone(), vec![item.ts.clone()]))
        .collect();
    let bodies = rt.block_on(api.messages_list(&ids)).unwrap();
    println!("hydrated {} channels", bodies.len());
    for (channel, messages) in &bodies {
        println!(
            "{channel}: {} messages, first text {} chars",
            messages.len(),
            messages.first().map(|m| m.text.len()).unwrap_or(0)
        );
    }
}

#[test]
fn parses_gif_featured_and_search_wire_payload() {
    let wire: GifListData = serde_json::from_value(serde_json::json!({
        "gifs": [
            {
                "id": "R7m04yMaGWVeE",
                "content_description": "Will Smith Mib GIF",
                "url": "https://giphy.com/gifs/will-smith-R7m04yMaGWVeE",
                "media_formats": {
                    "tinygif": { "url": "https://media2.giphy.com/media/R7m04yMaGWVeE/200w.webp" },
                    "tinygifpreview": { "url": "https://media2.giphy.com/media/R7m04yMaGWVeE/200w_s.gif" },
                    "gif": { "url": "https://media2.giphy.com/media/R7m04yMaGWVeE/giphy.webp" }
                }
            },
            {
                "id": "empty_should_skip",
                "content_description": "",
                "url": "",
                "media_formats": {}
            }
        ]
    }))
    .unwrap();
    let parsed = parse_gif_list(wire);
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].id, "R7m04yMaGWVeE");
    assert_eq!(parsed[0].title, "Will Smith Mib GIF");
    assert_eq!(
        parsed[0].still_url,
        "https://media2.giphy.com/media/R7m04yMaGWVeE/200w_s.gif"
    );
    assert_eq!(
        parsed[0].preview_url,
        "https://media2.giphy.com/media/R7m04yMaGWVeE/200w.webp"
    );
    assert_eq!(
        parsed[0].url,
        "https://giphy.com/gifs/will-smith-R7m04yMaGWVeE"
    );
    let attachment = parsed[0].to_attachment();
    assert_eq!(
        serde_json::to_value(&attachment).unwrap(),
        serde_json::json!({
            "fallback": "shared a GIF",
            "blocks": [{
                "type": "image",
                "image_url": "https://media2.giphy.com/media/R7m04yMaGWVeE/200w.webp",
                "alt_text": "Will Smith Mib GIF",
                "title": {
                    "type": "plain_text",
                    "text": "GIF",
                    "emoji": true
                }
            }]
        })
    );
}

#[test]
fn parses_saved_list_wire_payload() {
    let wire: SavedListWire = serde_json::from_value(serde_json::json!({
        "saved_items": [
            {
                "item_id": "C01",
                "item_type": "message",
                "ts": "1700000001.000100",
                "date_created": 1700000010,
                "date_due": 1700003600,
                "date_completed": 0,
                "is_archived": false,
                "todo_state": "to_do"
            },
            {
                "item_id": "C02",
                "item_type": "message",
                "ts": "1700000002.000200",
                "date_completed": 1700001000,
                "is_archived": false,
                "todo_state": "completed"
            },
            {
                "item_id": "C03",
                "item_type": "message",
                "ts": "1700000003.000300",
                "is_archived": true
            },
            {
                "item_id": "F04",
                "item_type": "file",
                "ts": ""
            }
        ]
    }))
    .unwrap();
    let parsed = parse_saved_list(wire, "saved");
    assert_eq!(parsed.len(), 3);
    assert_eq!(parsed[0].channel.as_str(), "C01");
    assert_eq!(parsed[0].state, SavedItemState::InProgress);
    assert_eq!(parsed[0].date_due, Some(1700003600));
    assert_eq!(parsed[1].state, SavedItemState::Completed);
    assert_eq!(parsed[1].date_due, None);
    assert_eq!(parsed[2].state, SavedItemState::Archived);
}

#[test]
fn parses_client_app_commands_wire_payload() {
    let wire: AppCommandsWire = serde_json::from_value(serde_json::json!({
        "commands": [
            {
                "name": "/linear",
                "type": "app",
                "app": "AEMQ3Q4F4",
                "app_name": "Linear",
                "desc": "Create new issue on Linear",
                "usage": "issue title (optional) "
            },
            {
                "name": "/giphy",
                "type": "service",
                "service_name": "Giphy",
                "desc": "Post a random gif to channel",
                "usage": "[text]"
            },
            {
                "name": "/dm",
                "alias_of": "/msg",
                "type": "core"
            }
        ]
    }))
    .unwrap();
    let parsed = parse_app_commands(wire);
    assert_eq!(parsed.len(), 3);
    assert_eq!(parsed[0].name, "linear");
    assert_eq!(parsed[0].app_name.as_deref(), Some("Linear"));
    assert_eq!(parsed[0].usage, "issue title (optional)");
    assert_eq!(parsed[1].app_name.as_deref(), Some("Giphy"));
    assert_eq!(parsed[2].description, "Alias for /msg");
}

#[test]
fn parses_search_modules_messages_and_files_and_sparse_profile_update() {
    let msg_wire: SearchModulesMessagesWire = serde_json::from_value(serde_json::json!({
        "pagination": { "total_count": 42, "page": 1, "page_count": 3 },
        "items": [
            {
                "channel": { "id": "C01", "name": "general" },
                "messages": [
                    { "ts": "1700000001.000100", "user": "U01", "text": "hello \u{e000}deploy\u{e001} ok" }
                ]
            }
        ]
    }))
    .unwrap();
    let page = parse_search_modules_messages(msg_wire);
    assert_eq!(page.total, 42);
    assert_eq!(page.page_count, 3);
    assert_eq!(page.hits.len(), 1);
    assert_eq!(page.hits[0].channel.as_str(), "C01");
    assert_eq!(page.hits[0].channel_name.as_deref(), Some("general"));
    assert_eq!(page.hits[0].message.text, "hello deploy ok");

    let file_wire: SearchModulesFilesWire = serde_json::from_value(serde_json::json!({
        "pagination": { "total_count": 1, "page": 1, "page_count": 1 },
        "items": [
            { "id": "F01", "name": "plan.pdf", "title": "Q4 Plan", "mimetype": "application/pdf", "size": 1024 }
        ]
    }))
    .unwrap();
    let files = parse_search_modules_files(file_wire);
    assert_eq!(files.total, 1);
    assert_eq!(files.files.len(), 1);
    assert_eq!(files.files[0].id.as_str(), "F01");

    let update = ProfileUpdate {
        status_text: Some("In a meeting".into()),
        status_emoji: Some(":calendar:".into()),
        status_expiration: Some(1_700_003_600),
        ..Default::default()
    };
    let v = serde_json::to_value(&update).unwrap();
    assert_eq!(
        v,
        serde_json::json!({
            "status_text": "In a meeting",
            "status_emoji": ":calendar:",
            "status_expiration": 1_700_003_600
        })
    );
}

#[test]
fn parses_slack_list_schema_and_typed_records() {
    let file_obj = serde_json::json!({
        "title": "Launch tracker",
        "list_metadata": {
            "description": "Q4 tasks",
            "schema": [
                { "id": "Col1", "key": "name", "name": "Task", "type": "text", "is_primary_column": true },
                { "id": "Col2", "key": "assignee", "name": "Owner", "type": "user" },
                {
                    "id": "Col3",
                    "key": "status",
                    "name": "Status",
                    "type": "select",
                    "options": {
                        "choices": [
                            { "value": "OptDone", "label": "Done", "color": "green" },
                            { "value": "OptTodo", "label": "To do", "color": "gray" }
                        ]
                    }
                },
                { "id": "Col4", "key": "done", "name": "Shipped", "type": "checkbox" }
            ]
        }
    });
    let (title, desc, cols) = parse_list_schema(&file_obj);
    assert_eq!(title, "Launch tracker");
    assert_eq!(desc, "Q4 tasks");
    assert_eq!(cols.len(), 4);
    assert_eq!(cols[2].choices.len(), 2);
    assert_eq!(cols[2].choices[0].label, "Done");

    let records = serde_json::json!([
        {
            "id": "Rec1",
            "archived": false,
            "fields": [
                { "column_id": "Col1", "key": "name", "text": "Sign macOS build" },
                { "column_id": "Col2", "key": "assignee", "user": ["U1"] },
                { "column_id": "Col3", "key": "status", "select": ["OptDone"] },
                { "column_id": "Col4", "key": "done", "checkbox": true }
            ]
        },
        {
            "id": "RecArchived",
            "archived": true,
            "fields": []
        }
    ]);
    let rows = parse_list_records(records.as_array().unwrap());
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].cells.get("Col1"),
        Some(&SlackListCellValue::Text("Sign macOS build".into()))
    );
    assert_eq!(
        rows[0].cells.get("Col2"),
        Some(&SlackListCellValue::Users(vec![UserId::from("U1")]))
    );
    assert_eq!(
        rows[0].cells.get("Col3"),
        Some(&SlackListCellValue::Select(vec!["OptDone".into()]))
    );
    assert_eq!(
        rows[0].cells.get("Col4"),
        Some(&SlackListCellValue::Checkbox(true))
    );
}

#[test]
fn parses_file_collections_and_recently_deleted_wire() {
    let cols_wire: FileCollectionsListWire = serde_json::from_value(serde_json::json!({
        "collections": [
            { "id": "ColA", "name": "Specs", "emoji": ":page_facing_up:", "files": [{ "id": "F1" }, { "id": "F2" }] }
        ]
    }))
    .unwrap();
    let cols: Vec<FileCollection> = cols_wire.collections.into_iter().map(Into::into).collect();
    assert_eq!(cols.len(), 1);
    assert_eq!(cols[0].name, "Specs");
    assert_eq!(cols[0].file_ids, vec!["F1", "F2"]);

    let del_wire: RecentlyDeletedWire = serde_json::from_value(serde_json::json!({
        "files": [
            { "id": "F9", "title": "Old Canvas", "date_delete": 1763540000, "file_type": "quip" }
        ]
    }))
    .unwrap();
    assert_eq!(del_wire.files.len(), 1);
    assert_eq!(del_wire.files[0].id, "F9");
}

#[test]
fn parses_app_home_wire_and_view_action_container() {
    let wire: AppHomeWire = serde_json::from_value(serde_json::json!({
        "app_home": {
            "home_tab_enabled": true,
            "messages_tab_enabled": true
        },
        "home_view": {
            "id": "V0HOME1",
            "type": "home",
            "app_id": "A0APP1",
            "blocks": [
                { "type": "header", "text": { "type": "plain_text", "text": "Dashboard" } }
            ]
        }
    }))
    .unwrap();
    let home = wire.into_response();
    assert!(home.home_tab_enabled);
    assert!(home.messages_tab_enabled);
    assert_eq!(
        home.home_view.as_ref().and_then(|v| v["id"].as_str()),
        Some("V0HOME1")
    );

    let container = ActionContainer {
        channel: ChannelId::from("D0APP"),
        message_ts: Ts::default(),
        attachment_id: None,
        is_ephemeral: false,
        view_id: Some("V0HOME1".into()),
    };
    assert_eq!(
        container.json(),
        serde_json::json!({ "type": "view", "view_id": "V0HOME1" })
    );
}

#[test]
fn serializes_canvas_edit_ops_and_list_cell_inputs() {
    let replace = CanvasEditOp::Replace {
        section_id: Some("temp:C:abc123".into()),
        markdown: "- [x] Done".into(),
    }
    .to_change_json();
    assert_eq!(replace["operation"], "replace");
    assert_eq!(replace["section_id"], "temp:C:abc123");
    assert_eq!(replace["document_content"]["markdown"], "- [x] Done");

    let whole = CanvasEditOp::Replace {
        section_id: None,
        markdown: "# Title".into(),
    }
    .to_change_json();
    assert!(whole.get("section_id").is_none());

    let rename = CanvasEditOp::Rename {
        title: "Q4 Plan".into(),
    }
    .to_change_json();
    assert_eq!(rename["operation"], "rename");
    assert_eq!(rename["title_content"]["markdown"], "Q4 Plan");

    let text_cell = ListCellInput {
        column_id: "Col01".into(),
        value: ListCellInputValue::Text("Ship release".into()),
    }
    .to_json(Some("Rec01"));
    assert_eq!(text_cell["row_id"], "Rec01");
    assert_eq!(text_cell["column_id"], "Col01");
    assert_eq!(
        text_cell["rich_text"][0]["elements"][0]["elements"][0]["text"],
        "Ship release"
    );

    let chk_cell = ListCellInput {
        column_id: "Col02".into(),
        value: ListCellInputValue::Checkbox(true),
    }
    .to_json(Some("Rec01"));
    assert_eq!(chk_cell["checkbox"], true);

    let sel_cell = ListCellInput {
        column_id: "Col03".into(),
        value: ListCellInputValue::Select(vec!["OptDone".into()]),
    }
    .to_json(None);
    assert!(sel_cell.get("row_id").is_none());
    assert_eq!(sel_cell["select"], serde_json::json!(["OptDone"]));
}
