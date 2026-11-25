//! Unit tests for pure seams across `live/**` (no network, Keychain, hardware, or `$HOME`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use slackless_core::NotifyLevel;
use slackless_core::api::{
    ApiError, CanvasAccessLevel, CanvasCommentReply, CanvasCommentThread, CanvasEditOp, CanvasMeta,
    ChimeAttendee, ChimeJoinData, ChimeMediaPlacement, ChimeMeeting, ClientCounts,
    ConversationCounts, HuddleCanvasRef, RoomJoinResponse, SavedItem, SavedItemState,
    SlackListCellValue, SlackListColumn, SlackListData, SlackListRow, ThreadCounts,
};
use slackless_core::domain::canvas::CanvasCommentFilter;
use slackless_core::domain::lists::{cell_to_text, list_to_csv, list_to_json_value, list_to_tsv};
use slackless_core::huddle::{
    AudioDeviceInfo, AudioDevices, CameraDeviceInfo, HuddlePhase, HuddleSessionSnapshot,
    ScreenSource,
};
use slackless_core::store::Store;
use slackless_core::types::event::{
    Hello, ItemRef, MessageDeleted, ReactionEvent, ThreadMarkedEvent, ThreadSubscription,
};
use slackless_core::types::{
    BotId, ChannelId, Conversation, File, FileId, HuddleRoom, Message, MessageEvent, Profile,
    SlackEvent, TeamId, Ts, User, UserId,
};

use crate::ListReadFormat;
use crate::archive::Archive;

use super::*;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let n = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "slackless-cli-live-test-{}-{}-{n}",
            label,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn formats_list_as_json_csv_and_tsv() {
    let mut cells = HashMap::new();
    cells.insert("Col1".into(), SlackListCellValue::Text("Ship, v1".into()));
    cells.insert("Col2".into(), SlackListCellValue::Checkbox(true));
    let data = SlackListData {
        file_id: "F0LIST1".into(),
        title: "Launch".into(),
        description: String::new(),
        columns: vec![
            SlackListColumn {
                id: "Col1".into(),
                key: "name".into(),
                name: "Task".into(),
                kind: "text".into(),
                is_primary: true,
                choices: Vec::new(),
            },
            SlackListColumn {
                id: "Col2".into(),
                key: "done".into(),
                name: "Done".into(),
                kind: "checkbox".into(),
                is_primary: false,
                choices: Vec::new(),
            },
        ],
        rows: vec![SlackListRow {
            id: "Rec1".into(),
            cells,
        }],
    };

    let id_user = |u: &UserId| u.to_string();
    let json = list_to_json_value(&data, id_user);
    assert_eq!(json["count"], 1);
    assert_eq!(json["items"][0]["Task"], "Ship, v1");
    assert_eq!(json["items"][0]["Done"], true);

    let csv = list_to_csv(&data, id_user);
    assert!(csv.contains("Task,Done\n"));
    assert!(csv.contains("\"Ship, v1\",true\n"));

    let tsv = list_to_tsv(&data, id_user);
    assert!(tsv.contains("id\tTask\tDone\n"));
    assert!(tsv.contains("Rec1\tShip, v1\ttrue\n"));
}

#[test]
fn cell_to_text_covers_all_variants_and_list_escaping() {
    let col = SlackListColumn {
        id: "C1".into(),
        key: "k".into(),
        name: "Col".into(),
        kind: "text".into(),
        is_primary: false,
        choices: vec![],
    };
    let resolve_user = |u: &UserId| u.to_string();

    assert_eq!(cell_to_text(None, &col, &resolve_user), "");
    assert_eq!(
        cell_to_text(Some(&SlackListCellValue::Empty), &col, &resolve_user),
        ""
    );
    assert_eq!(
        cell_to_text(
            Some(&SlackListCellValue::Date("2026-11-25".into())),
            &col,
            &resolve_user
        ),
        "2026-11-25"
    );
    assert_eq!(
        cell_to_text(
            Some(&SlackListCellValue::Number("42.5".into())),
            &col,
            &resolve_user
        ),
        "42.5"
    );
    assert_eq!(
        cell_to_text(
            Some(&SlackListCellValue::Users(vec![
                UserId::from("U1"),
                UserId::from("U2"),
            ])),
            &col,
            &resolve_user
        ),
        "U1; U2"
    );
    assert_eq!(
        cell_to_text(
            Some(&SlackListCellValue::Select(vec![
                "P0".into(),
                "Backend".into(),
            ])),
            &col,
            &resolve_user
        ),
        "P0; Backend"
    );
    assert_eq!(
        cell_to_text(
            Some(&SlackListCellValue::Checkbox(false)),
            &col,
            &resolve_user
        ),
        "false"
    );
    assert_eq!(
        cell_to_text(
            Some(&SlackListCellValue::Link {
                label: "Spec".into(),
                url: "https://example.com".into(),
            }),
            &col,
            &resolve_user
        ),
        "Spec (https://example.com)"
    );
    assert_eq!(
        cell_to_text(
            Some(&SlackListCellValue::Link {
                label: String::new(),
                url: "https://example.com".into(),
            }),
            &col,
            &resolve_user
        ),
        "https://example.com"
    );

    // Key fallback when cell is stored under col.key, plus RFC-4180 CR/LF CSV quoting & TSV sanitization.
    let mut cells = HashMap::new();
    cells.insert(
        "notes_key".into(),
        SlackListCellValue::Text("Say \"hi\"\tline1\r\nline2".into()),
    );
    let data = SlackListData {
        file_id: "F2".into(),
        title: "T".into(),
        description: "D".into(),
        columns: vec![SlackListColumn {
            id: "ColMissing".into(),
            key: "notes_key".into(),
            name: "Notes, \"Raw\"".into(),
            kind: "text".into(),
            is_primary: true,
            choices: vec![],
        }],
        rows: vec![SlackListRow {
            id: "R1".into(),
            cells,
        }],
    };

    let json = list_to_json_value(&data, resolve_user);
    assert_eq!(
        json["items"][0]["Notes, \"Raw\""],
        "Say \"hi\"\tline1\r\nline2"
    );

    let csv = list_to_csv(&data, resolve_user);
    assert_eq!(
        csv,
        "\"Notes, \"\"Raw\"\"\"\n\"Say \"\"hi\"\"\tline1\r\nline2\"\n"
    );

    let tsv = list_to_tsv(&data, resolve_user);
    assert_eq!(tsv, "id\tNotes, \"Raw\"\nR1\tSay \"hi\" line1  line2\n");
}

#[test]
fn format_watch_event_handles_messages_reactions_threads_filters_and_ignored() {
    let ch1 = ChannelId::from("C1");
    let ch2 = ChannelId::from("C2");

    // User message matching filter.
    let user_msg = SlackEvent::Message(MessageEvent::Message(Box::new(Message {
        channel: Some(ch1.clone()),
        ts: Ts::from("1700000001.000100"),
        thread_ts: Some(Ts::from("1700000000.000001")),
        user: Some(UserId::from("U1")),
        text: "hello".into(),
        ..Message::default()
    })));
    let (json, human) = format_watch_event(&user_msg, Some(&ch1)).expect("matches C1");
    assert_eq!(json["type"], "message");
    assert_eq!(json["channel"], "C1");
    assert_eq!(json["user"], "U1");
    assert_eq!(json["thread_ts"], "1700000000.000001");
    assert_eq!(human, "message\tC1\t1700000001.000100\tU1: hello\n");
    assert!(format_watch_event(&user_msg, Some(&ch2)).is_none());

    // Bot message and unknown author fallback.
    let bot_msg = SlackEvent::Message(MessageEvent::Message(Box::new(Message {
        channel: Some(ch1.clone()),
        ts: Ts::from("1700000002.000100"),
        bot_id: Some(BotId::from("B9")),
        text: "deploy ok".into(),
        ..Message::default()
    })));
    let (_, human) = format_watch_event(&bot_msg, None).expect("no filter");
    assert_eq!(human, "message\tC1\t1700000002.000100\tB9: deploy ok\n");

    let unknown_msg = SlackEvent::Message(MessageEvent::Message(Box::new(Message {
        ts: Ts::from("1700000003.000100"),
        text: "ghost".into(),
        ..Message::default()
    })));
    let (_, human) = format_watch_event(&unknown_msg, None).expect("unknown author");
    assert_eq!(human, "message\t\t1700000003.000100\tunknown: ghost\n");
    // Channel-less message is rejected when a channel filter is active.
    assert!(format_watch_event(&unknown_msg, Some(&ch1)).is_none());

    // ReactionAdded with channel filter and missing optional fields.
    let rx_ev = SlackEvent::ReactionAdded(ReactionEvent {
        user: UserId::from("U2"),
        reaction: "tada".into(),
        item: ItemRef {
            r#type: "message".into(),
            channel: Some(ch1.clone()),
            ts: Some(Ts::from("1700000001.000100")),
            ..ItemRef::default()
        },
        ..ReactionEvent::default()
    });
    let (json, human) = format_watch_event(&rx_ev, Some(&ch1)).expect("rx matches");
    assert_eq!(json["type"], "reaction_added");
    assert_eq!(json["reaction"], "tada");
    assert_eq!(human, "reaction_added\tC1\t1700000001.000100\tU2 :tada:\n");
    assert!(format_watch_event(&rx_ev, Some(&ch2)).is_none());

    let rx_missing_meta = SlackEvent::ReactionAdded(ReactionEvent {
        user: UserId::from("U3"),
        reaction: "eyes".into(),
        item: ItemRef::default(),
        ..ReactionEvent::default()
    });
    let (json, human) = format_watch_event(&rx_missing_meta, None).expect("unfiltered rx");
    assert_eq!(json["channel"], "");
    assert_eq!(json["ts"], "");
    assert_eq!(human, "reaction_added\t\t\tU3 :eyes:\n");
    assert!(format_watch_event(&rx_missing_meta, Some(&ch1)).is_none());

    // ThreadMarked with and without last_read.
    let tm_ev = SlackEvent::ThreadMarked(ThreadMarkedEvent {
        subscription: ThreadSubscription {
            channel: ch1.clone(),
            thread_ts: Ts::from("1700000000.000001"),
            last_read: Some(Ts::from("1700000005.000000")),
            active: true,
        },
        event_ts: None,
    });
    let (json, human) = format_watch_event(&tm_ev, Some(&ch1)).expect("tm matches");
    assert_eq!(json["type"], "thread_marked");
    assert_eq!(json["last_read"], "1700000005.000000");
    assert_eq!(json["active"], true);
    assert_eq!(
        human,
        "thread_marked\tC1\t1700000000.000001\tlast_read=1700000005.000000\n"
    );
    assert!(format_watch_event(&tm_ev, Some(&ch2)).is_none());

    let tm_none = SlackEvent::ThreadMarked(ThreadMarkedEvent {
        subscription: ThreadSubscription {
            channel: ch1.clone(),
            thread_ts: Ts::from("1700000000.000001"),
            last_read: None,
            active: false,
        },
        event_ts: None,
    });
    let (_, human) = format_watch_event(&tm_none, None).expect("tm none");
    assert_eq!(human, "thread_marked\tC1\t1700000000.000001\tlast_read=-\n");

    // Ignored events return None.
    assert!(format_watch_event(&SlackEvent::Hello(Hello::default()), None).is_none());
    let deleted_ev = SlackEvent::Message(MessageEvent::Deleted(MessageDeleted {
        channel: Some(ch1),
        deleted_ts: Ts::from("1700000001.000100"),
        previous_message: None,
        ts: None,
        event_ts: None,
    }));
    assert!(format_watch_event(&deleted_ev, None).is_none());

    // watch_reached_limit boundary checks.
    assert!(!watch_reached_limit(None, 0));
    assert!(!watch_reached_limit(None, 100));
    assert!(watch_reached_limit(Some(0), 0));
    assert!(!watch_reached_limit(Some(3), 2));
    assert!(watch_reached_limit(Some(3), 3));
    assert!(watch_reached_limit(Some(3), 4));
}

#[test]
fn summarize_unread_suppresses_muted_without_mentions_and_handles_caught_up() {
    let counts = ClientCounts {
        ims: vec![ConversationCounts {
            id: ChannelId::from("D1"),
            dm_count: 2,
            has_unreads: true,
            ..ConversationCounts::default()
        }],
        mpims: vec![ConversationCounts {
            id: ChannelId::from("G1"),
            has_unreads: false,
            ..ConversationCounts::default()
        }],
        channels: vec![
            // Muted with 0 mentions -> suppressed.
            ConversationCounts {
                id: ChannelId::from("C_MUTED_QUIET"),
                has_unreads: true,
                mention_count: 0,
                ..ConversationCounts::default()
            },
            // Muted WITH a mention -> kept!
            ConversationCounts {
                id: ChannelId::from("C_MUTED_PING"),
                has_unreads: true,
                mention_count: 1,
                ..ConversationCounts::default()
            },
            // Normal unread channel -> kept.
            ConversationCounts {
                id: ChannelId::from("C_ENG"),
                has_unreads: true,
                mention_count: 0,
                latest: Some(Ts::from("1700000010.000000")),
                ..ConversationCounts::default()
            },
        ],
        threads: ThreadCounts {
            has_unreads: true,
            mention_count: 3,
        },
        ..ClientCounts::default()
    };
    let mut levels = HashMap::new();
    levels.insert(ChannelId::from("C_MUTED_QUIET"), NotifyLevel::Muted);
    levels.insert(ChannelId::from("C_MUTED_PING"), NotifyLevel::Muted);
    levels.insert(ChannelId::from("C_ENG"), NotifyLevel::Mentions);

    let summary = summarize_unread(&counts, &levels, |id| format!("label:{id}"));
    let ids: Vec<&str> = summary
        .items
        .iter()
        .filter_map(|v| v["id"].as_str())
        .collect();
    assert_eq!(ids, vec!["D1", "C_MUTED_PING", "C_ENG"]);
    assert_eq!(summary.total_mentions, 3);
    assert!(summary.threads_unread);
    assert!(summary.human.contains("dm\tD1\tlabel:D1\tmentions=2\n"));
    assert!(summary.human.contains("threads\t-\tThreads\tmentions=3\n"));
    assert!(!summary.human.contains("C_MUTED_QUIET"));

    let json = format_unread_json(&TeamId::from("T1"), &summary, counts.threads.mention_count);
    assert_eq!(json["ok"], true);
    assert_eq!(json["workspace"], "T1");
    assert_eq!(json["unread_conversations"], 3);
    assert_eq!(json["total_mentions"], 3);
    assert_eq!(json["threads_unread"], true);
    assert_eq!(json["threads_mentions"], 3);

    // Caught-up case when all unreads are muted with 0 mentions and threads are clean.
    let empty = ClientCounts {
        channels: vec![ConversationCounts {
            id: ChannelId::from("C_MUTED_QUIET"),
            has_unreads: true,
            ..ConversationCounts::default()
        }],
        ..ClientCounts::default()
    };
    let caught_up = summarize_unread(&empty, &levels, |id| id.to_string());
    assert!(caught_up.items.is_empty());
    assert!(!caught_up.threads_unread);
    assert_eq!(caught_up.human, "all caught up\n");

    // Threads-only unread (mention_count > 0 even if has_unreads == false) does NOT say caught up.
    let threads_only = ClientCounts {
        threads: ThreadCounts {
            has_unreads: false,
            mention_count: 2,
        },
        ..ClientCounts::default()
    };
    let summary_t = summarize_unread(&threads_only, &levels, |id| id.to_string());
    assert!(summary_t.threads_unread);
    assert_eq!(summary_t.human, "threads\t-\tThreads\tmentions=2\n");
}

#[test]
fn status_normalization_download_sanitization_and_mark_thread_cursor() {
    // Status emoji normalization.
    assert_eq!(normalize_status_emoji(None), None);
    assert_eq!(
        normalize_status_emoji(Some("calendar")).as_deref(),
        Some(":calendar:")
    );
    assert_eq!(
        normalize_status_emoji(Some("  ::tada::  ")).as_deref(),
        Some(":tada:")
    );
    assert_eq!(normalize_status_emoji(Some(":::")).as_deref(), Some(""));
    assert_eq!(normalize_status_emoji(Some("   ")).as_deref(), Some(""));

    // Status expiration.
    assert_eq!(compute_status_expiration(None, 1000), None);
    assert_eq!(compute_status_expiration(Some(0), 1000), Some(0));
    assert_eq!(compute_status_expiration(Some(-60), 1000), Some(0));
    assert_eq!(compute_status_expiration(Some(300), 1000), Some(1300));
    assert_eq!(
        compute_status_expiration(Some(10), i64::MAX - 2),
        Some(i64::MAX)
    );

    // Download filename sanitization & URL fallback.
    assert_eq!(
        sanitize_download_name("F1", Some("../../etc/passwd"), None),
        "passwd"
    );
    assert_eq!(
        sanitize_download_name("F1", Some("dir/sub/report.pdf"), Some("Title")),
        "report.pdf"
    );
    assert_eq!(
        sanitize_download_name("F1", None, Some("../secret.txt")),
        "secret.txt"
    );
    assert_eq!(sanitize_download_name("F1", Some(".."), None), "F1.bin");
    assert_eq!(sanitize_download_name("F1", Some("/"), None), "F1.bin");
    assert_eq!(sanitize_download_name("F1", Some(""), None), "F1.bin");
    assert_eq!(sanitize_download_name("F1", None, None), "F1");

    let both_urls = File {
        id: FileId::from("F1"),
        name: Some("../../report.pdf".into()),
        url_private: Some("https://files.slack.com/private".into()),
        url_private_download: Some("https://files.slack.com/download".into()),
        ..File::default()
    };
    assert_eq!(
        select_download_url(&both_urls),
        Some("https://files.slack.com/download")
    );
    let (safe, default_dest) = resolve_download_dest("F1", &both_urls, None);
    assert_eq!(safe, "report.pdf");
    assert_eq!(default_dest, PathBuf::from("report.pdf"));
    let (safe2, custom_dest) =
        resolve_download_dest("F1", &both_urls, Some(Path::new("/tmp/custom.pdf")));
    assert_eq!(safe2, "report.pdf");
    assert_eq!(custom_dest, PathBuf::from("/tmp/custom.pdf"));

    let private_only = File {
        id: FileId::from("F2"),
        url_private: Some("https://files.slack.com/private".into()),
        ..File::default()
    };
    assert_eq!(
        select_download_url(&private_only),
        Some("https://files.slack.com/private")
    );
    assert_eq!(select_download_url(&File::default()), None);

    // mark-thread cursor selection.
    assert_eq!(
        mark_thread_cursor("1700.001", None, false),
        Ts::from("1700.001")
    );
    assert_eq!(
        mark_thread_cursor("1700.001", Some("1700.005"), false),
        Ts::from("1700.005")
    );
    assert_eq!(mark_thread_cursor("1700.001", None, true), Ts::from("0"));
    assert_eq!(
        mark_thread_cursor("1700.001", Some("1700.002"), true),
        Ts::from("1700.002")
    );
}

#[test]
fn classify_api_error_distinguishes_auth_expired_from_other_errors() {
    for code in [
        "invalid_auth",
        "not_authed",
        "token_revoked",
        "token_expired",
        "account_inactive",
    ] {
        let err = ApiError::from_slack_error(code);
        let (cli_code, msg) = classify_api_error(&err);
        assert_eq!(cli_code, "session_rejected");
        assert!(msg.contains("slackless auth login"));
        assert!(msg.contains(code));
    }

    let other = ApiError::from_slack_error("channel_not_found");
    let (code, msg) = classify_api_error(&other);
    assert_eq!(code, "request_failed");
    assert!(msg.contains("channel_not_found"));

    let rl = ApiError::RateLimited {
        retry_after: Duration::from_secs(5),
    };
    let (code, msg) = classify_api_error(&rl);
    assert_eq!(code, "request_failed");
    assert!(msg.contains("rate limited"));

    let untrusted = ApiError::UntrustedUrl("http://evil.example".into());
    let (code, _) = classify_api_error(&untrusted);
    assert_eq!(code, "request_failed");
}

#[test]
fn resolve_channel_with_handles_missing_archive_and_isolated_sqlite_archive() {
    let ws = TeamId::from("T1");

    // Without an archive, strips leading `#` and returns no label.
    assert_eq!(
        resolve_channel_with(None, &ws, "#general"),
        (ChannelId::from("general"), None)
    );
    assert_eq!(
        resolve_channel_with(None, &ws, "C0123"),
        (ChannelId::from("C0123"), None)
    );
    assert_eq!(display_channel(&ChannelId::from("C0123"), None), "C0123");
    assert_eq!(
        display_channel(&ChannelId::from("C0123"), Some("#general")),
        "#general"
    );

    // With a temp-dir SQLite archive (never touching $HOME).
    let dir = TempDir::new("resolve-channel");
    let db = dir.path().join("T1.sqlite");
    let mut store = Store::open(&db).expect("open store");
    store
        .upsert_users(
            &ws,
            &[User {
                id: UserId::from("U1"),
                name: "alice".into(),
                profile: Profile {
                    display_name: Some("Alice".into()),
                    ..Profile::default()
                },
                ..User::default()
            }],
        )
        .expect("upsert user");
    store
        .upsert_conversations(
            &ws,
            &[
                Conversation {
                    id: ChannelId::from("C_ENG"),
                    name: Some("engineering".into()),
                    is_channel: true,
                    is_member: true,
                    ..Conversation::default()
                },
                Conversation {
                    id: ChannelId::from("D_ALICE"),
                    is_im: true,
                    user: Some(UserId::from("U1")),
                    ..Conversation::default()
                },
            ],
        )
        .expect("upsert convs");
    drop(store);

    let archive = Archive::open_path(&db)
        .expect("open result")
        .expect("open archive");
    assert_eq!(
        resolve_channel_with(Some(&archive), &ws, "#engineering"),
        (ChannelId::from("C_ENG"), Some("#engineering".into()))
    );
    assert_eq!(
        resolve_channel_with(Some(&archive), &ws, "@alice"),
        (ChannelId::from("D_ALICE"), Some("D_ALICE".into()))
    );
    assert_eq!(
        resolve_channel_with(Some(&archive), &ws, "#unknown"),
        (ChannelId::from("unknown"), None)
    );
}

#[test]
fn saved_mark_thread_and_files_formatters_cover_edge_cases() {
    let (json, human) = format_mark_thread_output(
        &ChannelId::from("C1"),
        Some("#eng"),
        &Ts::from("1700.1"),
        &Ts::from("0"),
        false,
    );
    assert_eq!(json["ok"], true);
    assert_eq!(json["read"], false);
    assert_eq!(json["ts"], "0");
    assert_eq!(human, "marked thread 1700.1 in #eng as unread\n");

    let (_, human_read) = format_mark_thread_output(
        &ChannelId::from("C1"),
        None,
        &Ts::from("1700.1"),
        &Ts::from("1700.9"),
        true,
    );
    assert_eq!(human_read, "marked thread 1700.1 in C1 as read\n");

    // Saved list formatting with preview and missing message body.
    let saved_items = vec![
        SavedItem {
            channel: ChannelId::from("C1"),
            ts: Ts::from("1700.1"),
            state: SavedItemState::InProgress,
            date_due: Some(1_700_003_600),
            date_created: Some(1_700_000_000),
            message: Some(Message {
                text: "follow up".into(),
                ..Message::default()
            }),
        },
        SavedItem {
            channel: ChannelId::from("C2"),
            ts: Ts::from("1700.2"),
            state: SavedItemState::Completed,
            date_due: None,
            date_created: None,
            message: None,
        },
    ];
    assert_eq!(normalize_saved_filter("in_progress"), "saved");
    assert_eq!(normalize_saved_filter(""), "saved");
    assert_eq!(normalize_saved_filter("saved"), "saved");
    assert_eq!(normalize_saved_filter("completed"), "completed");
    assert_eq!(normalize_saved_filter("archived"), "archived");
    let (json, human) = format_saved_list("saved", &saved_items);
    assert_eq!(json["filter"], "saved");
    assert_eq!(json["count"], 2);
    assert_eq!(json["items"][0]["state"], "inprogress");
    assert_eq!(json["items"][0]["text"], "follow up");
    assert_eq!(json["items"][1]["text"], serde_json::Value::Null);
    assert!(human.contains("C1\t1700.1\tInProgress\tfollow up\n"));
    assert!(human.contains("C2\t1700.2\tCompleted\t(no preview)\n"));

    // Files list clamping & formatting fallbacks.
    assert_eq!(clamp_files_limit(0), 1);
    assert_eq!(clamp_files_limit(50), 50);
    assert_eq!(clamp_files_limit(999), 200);

    let files = vec![
        File {
            id: FileId::from("F1"),
            name: Some("raw.pdf".into()),
            title: Some("Q4 Plan".into()),
            filetype: Some("pdf".into()),
            size: Some(2048),
            user: Some(UserId::from("U1")),
            permalink: Some("https://slack.com/files/F1".into()),
            ..File::default()
        },
        File {
            id: FileId::from("F2"),
            name: Some("notes.txt".into()),
            title: None,
            filetype: None,
            size: None,
            ..File::default()
        },
        File {
            id: FileId::from("F3"),
            ..File::default()
        },
    ];
    let (json, human) = format_files_list(&files);
    assert_eq!(json["count"], 3);
    assert_eq!(json["files"][0]["user"], "U1");
    assert_eq!(json["files"][2]["user"], serde_json::Value::Null);
    assert_eq!(
        human,
        "F1\tpdf\t2048\tQ4 Plan\nF2\tfile\t-\tnotes.txt\nF3\tfile\t-\tF3\n"
    );
}

#[test]
fn canvas_list_and_backfill_seams_cover_argument_validation_and_formatting() {
    // Canvas edit op builder.
    assert_eq!(
        build_canvas_edit_op("hello".into(), Some("S1".into()), true),
        CanvasEditOp::InsertAtEnd {
            markdown: "hello".into()
        }
    );
    assert_eq!(
        build_canvas_edit_op("hello".into(), Some("S1".into()), false),
        CanvasEditOp::Replace {
            section_id: Some("S1".into()),
            markdown: "hello".into(),
        }
    );
    assert_eq!(
        build_canvas_edit_op("whole".into(), None, false),
        CanvasEditOp::Replace {
            section_id: None,
            markdown: "whole".into(),
        }
    );

    // Canvas read newline normalization.
    let (json, human) = format_canvas_read("F1", "markdown", "Spec", "# Title");
    assert_eq!(json["format"], "markdown");
    assert_eq!(json["content"], "# Title");
    assert_eq!(human, "# Title\n");
    let (_, human_nl) = format_canvas_read("F1", "html", "Spec", "<p>ok</p>\n");
    assert_eq!(human_nl, "<p>ok</p>\n");

    // Effective list format: global --json overrides --format.
    assert_eq!(
        effective_list_format(true, ListReadFormat::Csv),
        ListReadFormat::Json
    );
    assert_eq!(
        effective_list_format(false, ListReadFormat::Csv),
        ListReadFormat::Csv
    );
    assert_eq!(
        effective_list_format(false, ListReadFormat::Table),
        ListReadFormat::Table
    );

    // List primary column selection.
    assert_eq!(
        select_list_primary_column(&SlackListData::default()),
        "name"
    );
    let non_primary = SlackListData {
        columns: vec![
            SlackListColumn {
                id: "ColA".into(),
                is_primary: false,
                ..Default::default()
            },
            SlackListColumn {
                id: "ColB".into(),
                is_primary: false,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    assert_eq!(select_list_primary_column(&non_primary), "ColA");
    let with_primary = SlackListData {
        columns: vec![
            SlackListColumn {
                id: "ColA".into(),
                is_primary: false,
                ..Default::default()
            },
            SlackListColumn {
                id: "ColB".into(),
                is_primary: true,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    assert_eq!(select_list_primary_column(&with_primary), "ColB");

    // Backfill seams.
    assert_eq!(clamp_backfill_page_size(0), 1);
    assert_eq!(clamp_backfill_page_size(100), 100);
    assert_eq!(clamp_backfill_page_size(999), 200);
    assert!(!backfill_should_stop(0, 50));
    assert!(!backfill_should_stop(3, 2));
    assert!(backfill_should_stop(3, 3));

    let (json, human) = format_backfill_output(
        &TeamId::from("T1"),
        &ChannelId::from("C1"),
        Some("#eng"),
        2,
        150,
        true,
    );
    assert_eq!(json["pages"], 2);
    assert_eq!(json["messages"], 150);
    assert_eq!(json["reached_end"], true);
    assert_eq!(
        human,
        "backfilled #eng: 150 messages across 2 page(s) (complete=true)\n"
    );
}

#[test]
fn status_and_huddle_formatters_and_credential_builder_cover_all_branches() {
    // Status show: empty, text-only, emoji+text.
    let uid = UserId::from("U1");
    let (json, human) = format_status_show(&uid, &Profile::default());
    assert_eq!(json["status_text"], "");
    assert_eq!(json["status_emoji"], "");
    assert_eq!(json["status_expiration"], 0);
    assert_eq!(human, "(no status set)\n");

    let (json, human) = format_status_show(
        &uid,
        &Profile {
            status_text: Some("Deep focus".into()),
            status_emoji: Some(":headphones:".into()),
            status_expiration: Some(1_700_005_000),
            ..Profile::default()
        },
    );
    assert_eq!(json["status_expiration"], 1_700_005_000);
    assert_eq!(human, ":headphones: Deep focus\n");

    let (_, text_only) = format_status_show(
        &uid,
        &Profile {
            status_text: Some("Lunch".into()),
            ..Profile::default()
        },
    );
    assert_eq!(text_only, "Lunch\n");

    // Status set fallback to requested text when response profile omits status_text.
    let (json, human) = format_status_set(
        "OOO".into(),
        Profile {
            status_emoji: Some(":palm_tree:".into()),
            status_text: None,
            status_expiration: None,
            ..Profile::default()
        },
    );
    assert_eq!(json["status_text"], "OOO");
    assert_eq!(json["status_emoji"], ":palm_tree:");
    assert_eq!(human, "status set: :palm_tree: OOO\n");

    // Huddle devices, cameras, screens.
    let devices = AudioDevices {
        inputs: vec![
            AudioDeviceInfo {
                name: "Built-in Mic".into(),
                is_default: true,
            },
            AudioDeviceInfo {
                name: "USB Mic".into(),
                is_default: false,
            },
        ],
        outputs: vec![AudioDeviceInfo {
            name: "Speakers".into(),
            is_default: true,
        }],
    };
    let (json, human) = format_huddle_devices(&devices);
    assert_eq!(json["inputs"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        human,
        "INPUTS\n*\tBuilt-in Mic\n \tUSB Mic\nOUTPUTS\n*\tSpeakers\n"
    );

    let (json, human) = format_huddle_cameras(&[CameraDeviceInfo {
        index: 0,
        name: "FaceTime HD".into(),
    }]);
    assert_eq!(json["cameras"][0]["name"], "FaceTime HD");
    assert_eq!(human, "0\tFaceTime HD\n");

    let (json, human) = format_huddle_screens(&[
        ScreenSource {
            id: "mon-1".into(),
            name: "Main Display".into(),
            is_monitor: true,
            width: 2560,
            height: 1440,
        },
        ScreenSource {
            id: "win-2".into(),
            name: "Terminal".into(),
            is_monitor: false,
            width: 1200,
            height: 800,
        },
    ]);
    assert_eq!(json["sources"][0]["monitor"], true);
    assert_eq!(json["sources"][1]["monitor"], false);
    assert_eq!(
        human,
        "mon-1\tmonitor\t2560x1440\tMain Display\nwin-2\twindow\t1200x800\tTerminal\n"
    );

    // Huddle link fallback & credential assembly.
    let ws = TeamId::from("T1");
    let ch = ChannelId::from("C1");
    assert_eq!(
        fallback_huddle_link(&ws, &ch, None),
        "https://app.slack.com/huddle/T1/C1"
    );
    assert_eq!(
        fallback_huddle_link(&ws, &ch, Some("https://custom.link".into())),
        "https://custom.link"
    );

    let joined = RoomJoinResponse {
        call_id: "R1".into(),
        chime: ChimeJoinData {
            attendee: ChimeAttendee {
                attendee_id: "att-1".into(),
                external_user_id: "ext-1".into(),
                join_token: "tok-1".into(),
                ..Default::default()
            },
            meeting: ChimeMeeting {
                meeting_id: "mtg-1".into(),
                media_placement: ChimeMediaPlacement {
                    signaling_url: "wss://sig.test".into(),
                    audio_host_url: "audio.test:3478".into(),
                    turn_control_url: "https://turn.test".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
        },
        huddle: HuddleRoom {
            thread_root_ts: None,
            huddle_link: None,
            ..HuddleRoom::default()
        },
        canvas: Some(HuddleCanvasRef {
            root_thread_ts: Some(Ts::from("1700.5")),
            thread_channel_id: Some(ch.clone()),
            file_id: Some("F_CANVAS".into()),
        }),
    };
    let creds = build_huddle_credentials(&ws, &ch, joined);
    assert_eq!(creds.call_id, "R1");
    assert_eq!(creds.thread_root_ts, Some(Ts::from("1700.5")));
    assert_eq!(creds.canvas_file_id.as_deref(), Some("F_CANVAS"));
    assert_eq!(
        creds.huddle_link.as_deref(),
        Some("https://app.slack.com/huddle/T1/C1")
    );

    // Huddle state formatter with and without error suffix.
    let snap = HuddleSessionSnapshot {
        phase: HuddlePhase::Failed,
        channel: ch,
        call_id: "R1".into(),
        muted: true,
        packets_sent: 10,
        packets_received: 5,
        pongs_sent: 2,
        error: Some("ice timeout".into()),
        udp_blocked: true,
        ..Default::default()
    };
    let (json, human_err) = format_huddle_state("#eng", &snap, true);
    assert_eq!(json["phase"], "Failed");
    assert_eq!(json["udp_blocked"], true);
    assert_eq!(
        human_err,
        "huddle #eng\tFailed\ttx=10 rx=5 pongs=2\tice timeout\n"
    );
    let (_, human_no_err) = format_huddle_state("#eng", &snap, false);
    assert_eq!(human_no_err, "huddle #eng\tFailed\ttx=10 rx=5 pongs=2\n");
}

#[test]
fn canvas_share_target_and_post_validation_enforce_xor_dedup_and_non_empty() {
    let resolver = |ch: &str| match ch {
        "#eng" => ChannelId::from("C_ENG"),
        "#ops" => ChannelId::from("C_OPS"),
        other => ChannelId::from(other),
    };

    // Valid channels with resolution, trimming, and order-preserving deduplication.
    let target = parse_canvas_share_target(
        &[
            " #eng ".into(),
            "C_OPS".into(),
            "#ops".into(),
            "C_ENG".into(),
        ],
        &[],
        resolver,
    )
    .expect("valid channel target");
    assert_eq!(
        target,
        CanvasShareTarget::Channels(vec![ChannelId::from("C_ENG"), ChannelId::from("C_OPS")])
    );
    assert!(validate_canvas_share_post(&target, false).is_ok());
    assert!(validate_canvas_share_post(&target, true).is_ok());

    // Valid users with trimming and deduplication.
    let user_target =
        parse_canvas_share_target(&[], &[" U1 ".into(), "U2".into(), "U1".into()], resolver)
            .expect("valid user target");
    assert_eq!(
        user_target,
        CanvasShareTarget::Users(vec![UserId::from("U1"), UserId::from("U2")])
    );
    assert!(validate_canvas_share_post(&user_target, false).is_ok());
    assert!(
        validate_canvas_share_post(&user_target, true)
            .expect_err("--post with users must fail")
            .contains("--channel")
    );

    // XOR errors: neither or both specified.
    assert!(
        parse_canvas_share_target(&[], &[], resolver)
            .expect_err("empty targets must fail")
            .contains("at least one")
    );
    assert!(
        parse_canvas_share_target(&["C1".into()], &["U1".into()], resolver)
            .expect_err("both targets must fail")
            .contains("not both")
    );

    // Blank entries rejected.
    assert!(
        parse_canvas_share_target(&["   ".into()], &[], resolver)
            .expect_err("blank channel must fail")
            .contains("empty")
    );
    assert!(
        parse_canvas_share_target(&[], &[String::new()], resolver)
            .expect_err("blank user must fail")
            .contains("empty")
    );
}

#[test]
fn canvas_share_unshare_and_delete_formatters_cover_all_variants() {
    // Delete.
    let (json, human) = format_canvas_delete_result("F0123");
    assert_eq!(json["ok"], true);
    assert_eq!(json["canvas_id"], "F0123");
    assert_eq!(json["deleted"], true);
    assert_eq!(human, "deleted canvas F0123\n");

    // Share with channels, without --post.
    let ch_target = CanvasShareTarget::Channels(vec![ChannelId::from("C1"), ChannelId::from("C2")]);
    let (json, human) =
        format_canvas_share_result("F0123", CanvasAccessLevel::Write, &ch_target, &[], None);
    assert_eq!(json["ok"], true);
    assert_eq!(json["canvas_id"], "F0123");
    assert_eq!(json["access"], "write");
    assert_eq!(json["channels"], serde_json::json!(["C1", "C2"]));
    assert_eq!(json["users"], serde_json::json!([]));
    assert_eq!(json["posted"], false);
    assert_eq!(json["posted_channels"], serde_json::json!([]));
    assert_eq!(json["permalink"], serde_json::Value::Null);
    assert_eq!(human, "shared canvas F0123 (write) with channels: C1, C2\n");

    // Share with channels, WITH --post.
    let posted = vec![ChannelId::from("C1"), ChannelId::from("C2")];
    let (json, human) = format_canvas_share_result(
        "F0123",
        CanvasAccessLevel::Read,
        &ch_target,
        &posted,
        Some("https://acme.slack.com/docs/T1/F0123"),
    );
    assert_eq!(json["posted"], true);
    assert_eq!(json["posted_channels"], serde_json::json!(["C1", "C2"]));
    assert_eq!(json["permalink"], "https://acme.slack.com/docs/T1/F0123");
    assert_eq!(
        human,
        "shared canvas F0123 (read) with channels: C1, C2 (posted permalink to 2 channel(s))\n"
    );

    // Share with users.
    let user_target = CanvasShareTarget::Users(vec![UserId::from("U1")]);
    let (json, human) =
        format_canvas_share_result("F0123", CanvasAccessLevel::Owner, &user_target, &[], None);
    assert_eq!(json["access"], "owner");
    assert_eq!(json["channels"], serde_json::json!([]));
    assert_eq!(json["users"], serde_json::json!(["U1"]));
    assert_eq!(human, "shared canvas F0123 (owner) with users: U1\n");

    // Unshare channels & users.
    let (json, human) = format_canvas_unshare_result("F0123", &ch_target);
    assert_eq!(json["ok"], true);
    assert_eq!(json["channels"], serde_json::json!(["C1", "C2"]));
    assert_eq!(json["users"], serde_json::json!([]));
    assert_eq!(human, "unshared canvas F0123 from channels: C1, C2\n");

    let (json, human) = format_canvas_unshare_result("F0123", &user_target);
    assert_eq!(json["channels"], serde_json::json!([]));
    assert_eq!(json["users"], serde_json::json!(["U1"]));
    assert_eq!(human, "unshared canvas F0123 from users: U1\n");
}

#[test]
fn canvas_meta_formatter_covers_full_and_minimal_metadata() {
    let full = CanvasMeta {
        file_id: "F0999".into(),
        title: "Launch Spec".into(),
        permalink: Some("https://acme.slack.com/docs/T1/F0999".into()),
        editable: true,
        access: Some("write".into()),
        org_or_workspace_access: Some("read".into()),
        canvas_printing_enabled: false,
        canvas_creator_id: Some(UserId::from("U1")),
        editors: vec![UserId::from("U1"), UserId::from("U2")],
        quip_thread_id: Some("Q_ABC".into()),
        comment_channel_id: ChannelId::from("C0999"),
    };
    let (json, human) = format_canvas_meta(&full);
    assert_eq!(json["canvas_id"], "F0999");
    assert_eq!(json["title"], "Launch Spec");
    assert_eq!(json["permalink"], "https://acme.slack.com/docs/T1/F0999");
    assert_eq!(json["access"], "write");
    assert_eq!(json["editable"], true);
    assert_eq!(json["org_or_workspace_access"], "read");
    assert_eq!(json["canvas_printing_enabled"], false);
    assert_eq!(json["creator"], "U1");
    assert_eq!(json["editors"], serde_json::json!(["U1", "U2"]));
    assert_eq!(json["quip_thread_id"], "Q_ABC");
    assert_eq!(json["comment_channel_id"], "C0999");
    assert!(human.contains("Canvas:\tF0999\n"));
    assert!(human.contains("Title:\tLaunch Spec\n"));
    assert!(human.contains("Permalink:\thttps://acme.slack.com/docs/T1/F0999\n"));
    assert!(human.contains("Access:\twrite\n"));
    assert!(human.contains("Editable:\ttrue\n"));
    assert!(human.contains("Workspace access:\tread\n"));
    assert!(human.contains("Printing enabled:\tfalse\n"));
    assert!(human.contains("Creator:\tU1\n"));
    assert!(human.contains("Editors:\tU1, U2\n"));
    assert!(human.contains("Comment channel:\tC0999\n"));
    assert!(human.contains("Quip thread:\tQ_ABC\n"));

    let minimal = CanvasMeta {
        file_id: "F0100".into(),
        title: "Untitled".into(),
        comment_channel_id: ChannelId::from("C0100"),
        ..CanvasMeta::default()
    };
    let (json, human) = format_canvas_meta(&minimal);
    assert_eq!(json["permalink"], serde_json::Value::Null);
    assert_eq!(json["access"], serde_json::Value::Null);
    assert_eq!(json["creator"], serde_json::Value::Null);
    assert_eq!(json["editors"], serde_json::json!([]));
    assert_eq!(json["canvas_printing_enabled"], true);
    assert!(human.contains("Permalink:\t-\n"));
    assert!(human.contains("Access:\t-\n"));
    assert!(human.contains("Workspace access:\t-\n"));
    assert!(human.contains("Creator:\t-\n"));
    assert!(human.contains("Editors:\t-\n"));
    assert!(!human.contains("Quip thread:"));
}

#[test]
fn canvas_comments_filtering_formatting_posting_and_resolve_seams() {
    let threads = vec![
        CanvasCommentThread {
            root_ts: Ts::from("1700000001.000100"),
            author: Some(UserId::from("U1")),
            text: "First\nline\ttab".into(),
            reply_count: 1,
            replies: vec![CanvasCommentReply {
                ts: Ts::from("1700000002.000200"),
                author: None,
                text: "On it\nnow".into(),
            }],
            resolved: false,
        },
        CanvasCommentThread {
            root_ts: Ts::from("1700000010.000100"),
            author: None,
            text: "Already done".into(),
            reply_count: 0,
            replies: Vec::new(),
            resolved: true,
        },
    ];

    // Filter mode selection & filtering.
    assert_eq!(
        comment_filter_from_flags(false, false),
        CanvasCommentFilter::Open
    );
    assert_eq!(
        comment_filter_from_flags(true, false),
        CanvasCommentFilter::Resolved
    );
    assert_eq!(
        comment_filter_from_flags(false, true),
        CanvasCommentFilter::All
    );

    let open = filter_canvas_comments(&threads, CanvasCommentFilter::Open);
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].root_ts, Ts::from("1700000001.000100"));

    let resolved = filter_canvas_comments(&threads, CanvasCommentFilter::Resolved);
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].root_ts, Ts::from("1700000010.000100"));

    let all = filter_canvas_comments(&threads, CanvasCommentFilter::All);
    assert_eq!(all.len(), 2);

    // Comments formatting (JSON + human with replies and whitespace sanitization).
    let (json, human) = format_canvas_comments("F0123", CanvasCommentFilter::All, &all);
    assert_eq!(json["canvas_id"], "F0123");
    assert_eq!(json["comment_channel_id"], "C0123");
    assert_eq!(json["filter"], "all");
    assert_eq!(json["count"], 2);
    assert_eq!(json["threads"][0]["author"], "U1");
    assert_eq!(json["threads"][0]["resolved"], false);
    assert_eq!(
        json["threads"][0]["replies"][0]["author"],
        serde_json::Value::Null
    );
    assert_eq!(json["threads"][1]["resolved"], true);
    assert_eq!(
        human,
        "1700000001.000100\topen\tU1\treplies=1\tFirst line tab\n  1700000002.000200\treply\tunknown\tOn it now\n1700000010.000100\tresolved\tunknown\treplies=0\tAlready done\n"
    );

    // Empty comment list.
    let (empty_json, empty_human) = format_canvas_comments("F0123", CanvasCommentFilter::Open, &[]);
    assert_eq!(empty_json["count"], 0);
    assert_eq!(empty_json["threads"], serde_json::json!([]));
    assert_eq!(empty_human, "");

    // Comment argument validation & post formatting.
    assert_eq!(validate_canvas_comment_args("Looks good", None), Ok(None));
    assert_eq!(
        validate_canvas_comment_args("Reply", Some(" 1700.1 ")),
        Ok(Some(Ts::from("1700.1")))
    );
    assert!(validate_canvas_comment_args("   ", None).is_err());
    assert!(validate_canvas_comment_args("ok", Some("   ")).is_err());

    let cch = ChannelId::from("C0123");
    let (json, human) = format_canvas_comment_posted("F0123", &cch, None, &Ts::from("1700.5"));
    assert_eq!(json["ok"], true);
    assert_eq!(json["comment_channel_id"], "C0123");
    assert_eq!(json["thread_ts"], serde_json::Value::Null);
    assert_eq!(json["ts"], "1700.5");
    assert_eq!(human, "posted comment on canvas F0123 as 1700.5\n");

    let (json, human) = format_canvas_comment_posted(
        "F0123",
        &cch,
        Some(&Ts::from("1700.1")),
        &Ts::from("1700.9"),
    );
    assert_eq!(json["thread_ts"], "1700.1");
    assert_eq!(
        human,
        "replied to comment 1700.1 on canvas F0123 as 1700.9\n"
    );

    // Resolve action validation.
    assert_eq!(
        parse_canvas_resolve_action(Some(" 1700.1 "), false, false),
        Ok(CanvasResolveAction::ResolveOne(Ts::from("1700.1")))
    );
    assert_eq!(
        parse_canvas_resolve_action(Some("1700.1"), false, true),
        Ok(CanvasResolveAction::ReopenOne(Ts::from("1700.1")))
    );
    assert_eq!(
        parse_canvas_resolve_action(None, true, false),
        Ok(CanvasResolveAction::ResolveAll)
    );
    assert!(parse_canvas_resolve_action(None, false, false).is_err());
    assert!(parse_canvas_resolve_action(Some("1700.1"), true, false).is_err());
    assert!(parse_canvas_resolve_action(None, true, true).is_err());
    assert!(parse_canvas_resolve_action(Some("   "), false, false).is_err());

    // Resolve result formatting.
    let (json, human) = format_canvas_resolve_result(
        "F0123",
        &cch,
        &CanvasResolveAction::ResolveOne(Ts::from("1700.1")),
    );
    assert_eq!(json["resolved"], true);
    assert_eq!(json["all"], false);
    assert_eq!(json["thread_ts"], "1700.1");
    assert_eq!(human, "resolved comment thread 1700.1 on canvas F0123\n");

    let (json, human) = format_canvas_resolve_result(
        "F0123",
        &cch,
        &CanvasResolveAction::ReopenOne(Ts::from("1700.1")),
    );
    assert_eq!(json["resolved"], false);
    assert_eq!(json["all"], false);
    assert_eq!(human, "reopened comment thread 1700.1 on canvas F0123\n");

    let (json, human) =
        format_canvas_resolve_result("F0123", &cch, &CanvasResolveAction::ResolveAll);
    assert_eq!(json["resolved"], true);
    assert_eq!(json["all"], true);
    assert_eq!(json["thread_ts"], serde_json::Value::Null);
    assert_eq!(human, "resolved all comment threads on canvas F0123\n");
}

#[test]
fn classify_canvas_api_error_translates_canvas_codes_and_preserves_auth_expiry() {
    let auth = ApiError::from_slack_error("token_revoked");
    let (code, msg) = classify_canvas_api_error(&auth);
    assert_eq!(code, "session_rejected");
    assert!(msg.contains("slackless auth login"));

    let free_team = ApiError::from_slack_error("free_teams_cannot_create_standalone_canvases");
    let (code, msg) = classify_canvas_api_error(&free_team);
    assert_eq!(code, "request_failed");
    assert!(msg.contains("Free Slack workspaces"));

    let cant_del = ApiError::from_slack_error("cant_delete_file");
    let (code, msg) = classify_canvas_api_error(&cant_del);
    assert_eq!(code, "request_failed");
    assert!(msg.contains("cannot be deleted"));

    let unknown = ApiError::from_slack_error("some_other_error");
    let (code, msg) = classify_canvas_api_error(&unknown);
    assert_eq!(code, "request_failed");
    assert_eq!(msg, "some_other_error");
}

#[test]
fn canvas_html_fallback_never_swallows_auth_or_transport_errors() {
    assert!(should_fallback_canvas_html(&ApiError::from_slack_error(
        "not_allowed_token_type"
    )));
    assert!(should_fallback_canvas_html(&ApiError::from_slack_error(
        "unknown_method"
    )));
    assert!(!should_fallback_canvas_html(&ApiError::from_slack_error(
        "invalid_auth"
    )));
    assert!(!should_fallback_canvas_html(&ApiError::from_slack_error(
        "token_revoked"
    )));
    assert!(!should_fallback_canvas_html(&ApiError::RateLimited {
        retry_after: Duration::from_secs(5),
    }));
    assert!(!should_fallback_canvas_html(&ApiError::UntrustedUrl(
        "http://evil.example".into()
    )));
}

#[test]
fn backfill_stuck_cursor_guard_status_spacing_and_huddle_helpers() {
    // Stuck-cursor guard rejects empty and repeated cursors.
    assert_eq!(next_backfill_cursor(None, None), None);
    assert_eq!(next_backfill_cursor(None, Some(String::new())), None);
    assert_eq!(
        next_backfill_cursor(None, Some("cur1".into())),
        Some("cur1".into())
    );
    assert_eq!(
        next_backfill_cursor(Some("cur1"), Some("cur1".into())),
        None
    );
    assert_eq!(
        next_backfill_cursor(Some("cur1"), Some("cur2".into())),
        Some("cur2".into())
    );

    // Status set with empty emoji must not emit a double space.
    let (_, human) = format_status_set(
        "Heads down".into(),
        Profile {
            status_emoji: None,
            status_text: None,
            status_expiration: None,
            ..Profile::default()
        },
    );
    assert_eq!(human, "status set: Heads down\n");

    // Status show with emoji-only must not emit a trailing space.
    let (_, human) = format_status_show(
        &UserId::from("U1"),
        &Profile {
            status_emoji: Some(":coffee:".into()),
            status_text: None,
            ..Profile::default()
        },
    );
    assert_eq!(human, ":coffee:\n");

    // Whitespace-only huddle thread_root_ts falls back to canvas root_thread_ts or None.
    let ws = TeamId::from("T1");
    let ch = ChannelId::from("C1");
    let joined = RoomJoinResponse {
        call_id: "R2".into(),
        chime: ChimeJoinData::default(),
        huddle: HuddleRoom {
            thread_root_ts: Some(Ts::from("   ")),
            huddle_link: Some("   ".into()),
            ..HuddleRoom::default()
        },
        canvas: Some(HuddleCanvasRef {
            root_thread_ts: Some(Ts::from("  \t ")),
            thread_channel_id: Some(ch.clone()),
            file_id: None,
        }),
    };
    let creds = build_huddle_credentials(&ws, &ch, joined);
    assert_eq!(creds.thread_root_ts, None);
    assert_eq!(
        creds.huddle_link.as_deref(),
        Some("https://app.slack.com/huddle/T1/C1")
    );

    // format_ms_utc uses shared civil_from_days accurately.
    assert_eq!(format_ms_utc(0), "1970-01-01 00:00:00");
    assert_eq!(format_ms_utc(1_709_208_000_000), "2024-02-29 12:00:00");
}
