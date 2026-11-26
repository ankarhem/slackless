//! Wire deserialization, patchability, and parser edge-case tests across all API domains.

use super::*;
use crate::types::{ApiResponse, ChannelId, ConversationKind, SectionKind, TeamId, Ts, Usergroup};
use serde_json::json;

#[test]
fn auth_test_and_user_boot_tolerate_missing_null_and_drifted_fields() {
    let auth: AuthTest = serde_json::from_value(json!({
        "url": "https://acme.slack.com/",
        "team": "Acme",
        "team_id": "T1",
        "unknown_flag": 42
    }))
    .expect("auth.test parses");
    assert_eq!(auth.url.as_deref(), Some("https://acme.slack.com/"));
    assert_eq!(auth.team_id.as_ref().map(TeamId::as_str), Some("T1"));
    assert_eq!(auth.user_id, None);
    assert_eq!(auth.extra.get("unknown_flag"), Some(&json!(42)));

    let empty_auth: AuthTest = serde_json::from_str("{}").expect("empty auth.test");
    assert_eq!(empty_auth, AuthTest::default());

    // Drifted types, null fields, and corrupt array elements must never panic or fail the boot.
    let boot: UserBoot = serde_json::from_value(json!({
        "self": "not-an-object",
        "team": null,
        "channels": [
            { "id": "C1", "name": "general", "is_channel": true },
            "corrupt-entry",
            12345
        ],
        "ims": [
            null,
            { "id": "D1", "is_im": true }
        ],
        "is_open": "not-a-list",
        "starred": null,
        "channels_priority": "not-a-map",
        "unchanged_channel_ids": 99,
        "future_field": { "nested": true }
    }))
    .expect("drifted userBoot parses");
    assert!(boot.self_user.is_none());
    assert!(boot.team.is_none());
    assert_eq!(boot.channels.len(), 1);
    assert_eq!(boot.channels[0].id.as_str(), "C1");
    assert_eq!(boot.ims.len(), 1);
    assert_eq!(boot.ims[0].id.as_str(), "D1");
    assert!(boot.is_open.is_empty());
    assert!(boot.starred.is_empty());
    assert!(boot.channels_priority.is_empty());
    assert!(boot.unchanged_channel_ids.is_empty());
    assert!(boot.extra.contains_key("future_field"));
}

#[test]
fn client_counts_lenient_numbers_booleans_and_corrupt_entries() {
    let counts: ClientCounts = serde_json::from_value(json!({
        "channels": [
            {
                "id": "C1",
                "mention_count": "4",
                "dm_count": null,
                "has_unreads": "true",
                "last_read": 12345,
                "extra_metric": "kept"
            },
            "bad-row"
        ],
        "ims": null,
        "mpims": [{ "id": "G1", "dm_count": 2, "mention_count": 1, "has_unreads": 0 }],
        "threads": { "has_unreads": 1, "mention_count": "5" }
    }))
    .expect("drifted client.counts parses");

    assert_eq!(counts.channels.len(), 1);
    let c1 = &counts.channels[0];
    assert_eq!(c1.mention_count, 4);
    assert_eq!(c1.dm_count, 0);
    assert!(c1.has_unreads);
    assert!(c1.is_unread());
    assert_eq!(c1.effective_mentions(), 4);
    assert!(c1.extra.contains_key("extra_metric"));

    assert!(counts.ims.is_empty());
    assert_eq!(counts.mpims.len(), 1);
    assert_eq!(counts.mpims[0].effective_mentions(), 2);
    assert!(counts.mpims[0].is_unread());
    assert!(counts.threads.has_unreads);
    assert_eq!(counts.threads.mention_count, 5);
}

#[test]
fn channels_sections_pins_bookmarks_and_roles_tolerate_drift() {
    // Channel sections wire + merge_section deduplication across pages.
    let sec_wire: ChannelSectionsWire = serde_json::from_value(json!({
        "cursor": null,
        "channel_sections": [
            {
                "channel_section_id": "L1",
                "name": "Projects",
                "type": "standard",
                "emoji": "rocket",
                "next_channel_section_id": "",
                "channel_ids_page": { "channel_ids": ["C1", "C2"] }
            },
            {
                "channel_section_id": "L1",
                "name": "Projects",
                "channel_ids_page": { "channel_ids": ["C2", "C3"] }
            },
            {
                "channel_section_id": "L2",
                "name": null,
                "type": "unknown_future_kind",
                "next_channel_section_id": "L3",
                "channel_ids_page": null
            },
            {
                "channel_section_id": "L3",
                "type": null
            },
            "corrupt"
        ]
    }))
    .expect("channel sections parse");
    assert_eq!(sec_wire.cursor, None);
    let mut sections = Vec::new();
    for s in sec_wire.channel_sections {
        merge_section(&mut sections, s.into());
    }
    assert_eq!(sections.len(), 3);
    assert_eq!(
        sections[0].channel_ids,
        vec![
            ChannelId::from("C1"),
            ChannelId::from("C2"),
            ChannelId::from("C3")
        ]
    );
    assert_eq!(sections[0].next_id, None);
    assert_eq!(
        sections[1].kind,
        SectionKind::Other("unknown_future_kind".into())
    );
    assert_eq!(sections[1].next_id.as_deref(), Some("L3"));
    assert!(sections[1].channel_ids.is_empty());
    assert_eq!(sections[2].kind, SectionKind::Standard);

    // PinsList with drifted timestamps, nulls, and corrupt entries.
    let pins: PinsList = serde_json::from_value(json!({
        "items": [
            {
                "type": "message",
                "created": "not-an-int",
                "created_by": "U1",
                "message": { "ts": "1700.1", "text": "pinned!" },
                "file": null,
                "future_pin_field": true
            },
            {
                "type": "file",
                "created": 1700000000,
                "file": { "id": "F1", "name": "spec.pdf" }
            },
            "bad-pin"
        ]
    }))
    .expect("pins.list parses");
    assert_eq!(pins.items.len(), 2);
    assert_eq!(pins.items[0].created, None);
    assert_eq!(
        pins.items[0].message.as_ref().map(|m| m.text.as_str()),
        Some("pinned!")
    );
    assert_eq!(
        pins.items[1].file.as_ref().map(|f| f.id.as_str()),
        Some("F1")
    );

    // BookmarksList with folder (no link), child bookmark, and drifted fields.
    let bms: BookmarksList = serde_json::from_value(json!({
        "bookmarks": [
            {
                "id": "BkFolder",
                "channel_id": "C1",
                "title": "Docs",
                "type": "folder",
                "parent_id": ""
            },
            {
                "id": "BkLink",
                "channel_id": "C1",
                "title": "Runbook",
                "link": "https://example.com/runbook",
                "emoji": ":book:",
                "type": "link",
                "parent_id": "BkFolder",
                "date_created": "bad-ts",
                "extra_k": 1
            },
            123
        ]
    }))
    .expect("bookmarks.list parses");
    assert_eq!(bms.bookmarks.len(), 2);
    assert_eq!(bms.bookmarks[0].link, None);
    assert_eq!(bms.bookmarks[1].date_created, None);
    assert_eq!(bms.bookmarks[1].parent_id.as_deref(), Some("BkFolder"));
    assert!(bms.bookmarks[1].extra.contains_key("extra_k"));

    // RoleAssignmentsWire & listing_params.
    let roles: RoleAssignmentsWire = serde_json::from_value(json!({
        "role_assignments": [
            { "role_id": CHANNEL_MANAGER_ROLE, "users": ["U1", "U2"] },
            { "role_id": "OTHER", "users": ["U3"] }
        ]
    }))
    .expect("role assignments parse");
    assert_eq!(roles.role_assignments.len(), 2);
    assert_eq!(roles.role_assignments[0].users.len(), 2);

    let params = listing_params(
        &ListOpts {
            kinds: vec![
                ConversationKind::PublicChannel,
                ConversationKind::PrivateChannel,
                ConversationKind::Im,
                ConversationKind::Mpim,
            ],
            exclude_archived: true,
            limit: Some(250),
        },
        Some("cur_1".into()),
    );
    assert_eq!(
        params,
        vec![
            ("types", "public_channel,private_channel,im,mpim".to_owned()),
            ("exclude_archived", "true".to_owned()),
            ("limit", "250".to_owned()),
            ("cursor", "cur_1".to_owned()),
        ]
    );
    assert!(listing_params(&ListOpts::default(), None).is_empty());
}

#[test]
fn messaging_threads_scheduled_drafts_and_gifs_edge_cases() {
    // HistoryOpts::push
    let mut params = Vec::new();
    HistoryOpts {
        oldest: Some(Ts::from("100.1")),
        latest: Some(Ts::from("200.2")),
        inclusive: true,
        limit: Some(50),
    }
    .push(&mut params);
    assert_eq!(
        params,
        vec![
            ("oldest", "100.1".to_owned()),
            ("latest", "200.2".to_owned()),
            ("inclusive", "true".to_owned()),
            ("limit", "50".to_owned()),
        ]
    );

    // MessageContent::plain and push
    let content = MessageContent::plain("hello");
    let mut msg_params = Vec::new();
    content.push(&mut msg_params).expect("push message content");
    assert_eq!(msg_params[0], ("text", "hello".to_owned()));
    assert_eq!(msg_params[1].0, "blocks");
    assert_eq!(msg_params.len(), 2, "no attachments key when empty");

    // ThreadsView with null root_msg and corrupt reply elements.
    let tv: ThreadsView = serde_json::from_value(json!({
        "total_unread_replies": "7",
        "has_more": 1,
        "threads": [
            {
                "root_msg": null,
                "latest_replies": [{ "ts": "1.1", "text": "ok" }, "corrupt"],
                "unread_replies": null
            },
            "bad-thread"
        ]
    }))
    .expect("threads view parses");
    assert_eq!(tv.total_unread_replies, 7);
    assert!(tv.has_more);
    assert_eq!(tv.threads.len(), 1);
    assert_eq!(tv.threads[0].root.ts.as_str(), "");
    assert_eq!(tv.threads[0].latest_replies.len(), 1);
    assert!(tv.threads[0].unread_replies.is_empty());

    // ScheduledMessagesList empty & defaults + string post_at tolerance.
    let sched: ScheduledMessagesList =
        serde_json::from_value(json!({})).expect("empty scheduled list");
    assert!(sched.scheduled_messages.is_empty());
    let created_str: ScheduledMessageCreated = serde_json::from_value(json!({
        "scheduled_message_id": "Q9",
        "channel": "C1",
        "post_at": "1760010000"
    }))
    .expect("string post_at parses");
    assert_eq!(created_str.post_at, 1_760_010_000);

    // Draft timestamp padding (including non-ASCII safety) & scheduled vs unsent draft separation.
    assert_eq!(pad_draft_ts("1700000000.1"), "1700000000.1000000");
    assert_eq!(pad_draft_ts("1700000000"), "1700000000.0000000");
    assert_eq!(pad_draft_ts("1700000000.123456789"), "1700000000.1234567");
    assert_eq!(pad_draft_ts("1700000000.123456é89"), "1700000000.1234568");
    assert_eq!(pad_draft_ts("1700000000.éééé"), "1700000000.0000000");
    let mixed_drafts = json!({
        "drafts": [
            {
                "id": "DrUNSENT",
                "last_updated_ts": "1760000000.5",
                "date_scheduled": 0,
                "destinations": [{ "channel_id": "C1" }],
                "blocks": [{ "type": "rich_text", "elements": [{ "type": "rich_text_section", "elements": [{ "type": "text", "text": "wip draft" }] }] }]
            },
            {
                "id": "DrSCHED1",
                "last_updated_ts": "1760000001.25",
                "date_scheduled": "1760010000",
                "date_created": 1760000001,
                "destinations": [{ "channel_id": "C1" }],
                "blocks": [{ "type": "rich_text", "elements": [{ "type": "rich_text_section", "elements": [{ "type": "text", "text": "send at 11:00" }] }] }]
            },
            {
                "id": "DrSENT",
                "is_sent": true,
                "date_scheduled": 1760010000,
                "destinations": [{ "channel_id": "C2" }],
                "blocks": [{ "type": "rich_text", "elements": [{ "type": "rich_text_section", "elements": [{ "type": "text", "text": "already sent" }] }] }]
            }
        ]
    });
    let cloud = parse_cloud_drafts(&mixed_drafts);
    assert_eq!(cloud.len(), 1);
    assert_eq!(cloud[0].id, "DrUNSENT");
    let scheduled = parse_scheduled_drafts(&mixed_drafts, None);
    assert_eq!(scheduled.len(), 1);
    assert_eq!(scheduled[0].id, "DrSCHED1");
    assert_eq!(scheduled[0].post_at, 1_760_010_000);
    assert_eq!(scheduled[0].text, "send at 11:00");
    assert!(parse_scheduled_drafts(&mixed_drafts, Some(&ChannelId::from("C2"))).is_empty());
    assert_eq!(
        find_draft_last_updated_ts(&mixed_drafts, "DrSCHED1").as_deref(),
        Some("1760000001.2500000")
    );

    // Cloud drafts with missing/null/malformed shapes never panic.
    assert!(parse_cloud_drafts(&json!({})).is_empty());
    assert!(parse_cloud_drafts(&json!({ "drafts": null })).is_empty());
    assert!(
        parse_cloud_drafts(&json!({
            "drafts": [
                { "id": "D1", "destinations": "bad", "blocks": "bad" },
                { "id": "D2", "destinations": [{ "channel_id": "C1" }], "blocks": null }
            ]
        }))
        .is_empty()
    );

    // GIF fallback chains + blank title alt text.
    let gif_wire: GifListData = serde_json::from_value(json!({
        "gifs": [
            {
                "id": "g1",
                "content_description": "   ",
                "url": "",
                "media_formats": {
                    "gif": { "url": "https://gif.test/full.gif" },
                    "gifpreview": { "url": "https://gif.test/still.png" }
                }
            },
            {
                "id": "g2",
                "content_description": "Fallback still",
                "url": "",
                "media_formats": {
                    "tinygif": { "url": "https://gif.test/tiny.webp" }
                }
            }
        ]
    }))
    .expect("gif list parses");
    let gifs = parse_gif_list(gif_wire);
    assert_eq!(gifs.len(), 2);
    assert_eq!(gifs[0].preview_url, "https://gif.test/full.gif");
    assert_eq!(gifs[0].still_url, "https://gif.test/still.png");
    assert_eq!(gifs[0].url, "https://gif.test/full.gif");
    let att = gifs[0].to_attachment();
    let att_val = serde_json::to_value(&att).expect("serialize gif attachment");
    assert_eq!(att_val["blocks"][0]["alt_text"], "GIF");
    assert_eq!(gifs[1].still_url, "https://gif.test/tiny.webp");
    assert_eq!(gifs[1].url, "https://gif.test/tiny.webp");
}

#[test]
fn activity_feed_and_saved_list_drifted_and_edge_shapes() {
    // Channel activity, inline message body, missing thread_entry, missing ts, mention aliases.
    let feed_wire: ActivityFeedWire = serde_json::from_value(json!({
        "items": [
            {
                "is_unread": 1,
                "feed_ts": "1700.1",
                "key": "channel-C9-1700.1",
                "item": {
                    "type": "channel",
                    "bundle_info": {
                        "unread_count": "4",
                        "payload": {
                            "channel_entry": {
                                "latest_message": {
                                    "channel": "C9",
                                    "ts": "1700.1",
                                    "user": "U5",
                                    "text": "inline body"
                                }
                            }
                        }
                    }
                }
            },
            {
                "is_unread": false,
                "feed_ts": "1700.2",
                "key": "keyword-C1-1700.2",
                "item": {
                    "type": "keyword",
                    "message": { "channel": "C1", "ts": "1700.2", "author_user_id": "U2" }
                }
            },
            {
                "is_unread": false,
                "feed_ts": "1700.3",
                "key": "at_user_group-C1-1700.3",
                "item": {
                    "type": "at_user_group",
                    "message": { "channel": "C1", "ts": "1700.3" }
                }
            },
            {
                "is_unread": true,
                "feed_ts": "1700.4",
                "key": "thread_v2-missing",
                "item": { "type": "thread_v2", "bundle_info": { "payload": {} } }
            },
            {
                "is_unread": true,
                "feed_ts": "1700.5",
                "key": "missing_item"
            },
            {
                "is_unread": true,
                "feed_ts": "1700.6",
                "key": "no_ts",
                "item": { "type": "at_everyone", "message": { "channel": "C1" } }
            }
        ]
    }))
    .expect("activity feed parses");
    let rows: Vec<ActivityItem> = feed_wire.items.into_iter().filter_map(row).collect();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].kind, ActivityKind::Channel);
    assert_eq!(rows[0].count, 4);
    assert_eq!(
        rows[0].message.as_ref().map(|m| m.text.as_str()),
        Some("inline body")
    );
    assert_eq!(rows[1].kind, ActivityKind::Mention);
    assert_eq!(rows[1].entry().kind, "keyword");
    assert_eq!(rows[2].kind, ActivityKind::Mention);
    assert_eq!(rows[2].entry().kind, "at_user_group");

    // Hyphen-less key falls back to ActivityKind::type_name().
    let fallback_item = ActivityItem {
        kind: ActivityKind::Reaction,
        key: "opaque".into(),
        feed_ts: Ts::from("1.0"),
        ..Default::default()
    };
    assert_eq!(fallback_item.entry().kind, "message_reaction");
    assert_eq!(ActivityKind::Unknown.type_name(), "unknown");
    assert_eq!(ActivityKind::Dm.type_name(), "dm");
    assert_eq!(ActivityKind::App.type_name(), "bot_dm_bundle");
    assert_eq!(ActivityKind::Channel.type_name(), "channel");

    // SavedListWire fallback from empty `saved_items` -> `items`, filter override, lenient bool.
    let saved_wire: SavedListWire = serde_json::from_value(json!({
        "saved_items": [],
        "items": [
            { "item_id": "C1", "ts": "1700.1", "date_created": 0, "date_due": 0 },
            { "item_id": "C2", "ts": "1700.2", "is_archived": "true" },
            { "item_id": "", "ts": "1700.3" },
            { "item_id": "C3", "item_type": "message", "ts": "" },
            "corrupt"
        ]
    }))
    .expect("saved list parses");
    let completed = parse_saved_list(saved_wire, "completed");
    assert_eq!(completed.len(), 2);
    assert_eq!(completed[0].state, SavedItemState::Completed);
    assert_eq!(completed[0].date_created, None);
    assert_eq!(completed[0].date_due, None);
    assert_eq!(completed[1].state, SavedItemState::Archived);

    assert_eq!(SavedMark::Completed.as_str(), "completed");
    assert_eq!(SavedMark::Uncompleted.as_str(), "uncompleted");
    assert_eq!(SavedMark::Archived.as_str(), "archived");
    assert_eq!(SavedMark::Unarchived.as_str(), "unarchived");
}

#[test]
fn files_collections_favorites_and_recently_deleted_tolerate_null_and_corrupt_entries() {
    assert_eq!(
        FilesFilter::channel(ChannelId::from("C1")).channel,
        Some(ChannelId::from("C1"))
    );
    assert_eq!(FilesFilter::workspace(), FilesFilter::default());

    let files_wire: FilesWire = serde_json::from_value(json!({
        "files": [{ "id": "F1", "name": "a.txt" }, "corrupt", 99]
    }))
    .expect("files wire parses");
    assert_eq!(files_wire.files.len(), 1);

    let favs: FileFavoritesWire =
        serde_json::from_value(json!({ "file_ids": null })).expect("null favorites");
    assert!(favs.file_ids.is_empty());

    let cols: FileCollectionsListWire = serde_json::from_value(json!({
        "collections": [
            {
                "id": "Col1",
                "name": "Pinned",
                "files": [{ "id": "F1" }, { "id": "" }, "bad", { "id": "F2" }]
            },
            "bad-collection"
        ]
    }))
    .expect("collections parse");
    let converted: Vec<FileCollection> = cols.collections.into_iter().map(Into::into).collect();
    assert_eq!(converted.len(), 1);
    assert_eq!(converted[0].file_ids, vec!["F1", "F2"]);

    let deleted: RecentlyDeletedWire = serde_json::from_value(json!({
        "files": [{}, { "id": "F9" }, "bad-file"]
    }))
    .expect("recently deleted parses");
    assert_eq!(deleted.files.len(), 2);
    assert_eq!(deleted.files[0].id, "");
    assert_eq!(deleted.files[1].id, "F9");
}

#[test]
fn canvases_wire_defaults_and_all_edit_operations() {
    let created: CanvasCreated =
        serde_json::from_value(json!({ "canvas_id": null })).expect("null canvas_id");
    assert_eq!(created.canvas_id, "");

    let content: CanvasContentWire = serde_json::from_value(json!({})).expect("empty content");
    assert_eq!(content.content, "");

    let start = CanvasEditOp::InsertAtStart {
        markdown: "top".into(),
    }
    .to_change_json();
    assert_eq!(start["operation"], "insert_at_start");
    assert_eq!(start["document_content"]["markdown"], "top");

    let end = CanvasEditOp::InsertAtEnd {
        markdown: "bottom".into(),
    }
    .to_change_json();
    assert_eq!(end["operation"], "insert_at_end");

    let after = CanvasEditOp::InsertAfter {
        section_id: "S1".into(),
        markdown: "after".into(),
    }
    .to_change_json();
    assert_eq!(after["operation"], "insert_after");
    assert_eq!(after["section_id"], "S1");

    let before = CanvasEditOp::InsertBefore {
        section_id: "S2".into(),
        markdown: "before".into(),
    }
    .to_change_json();
    assert_eq!(before["operation"], "insert_before");
    assert_eq!(before["section_id"], "S2");

    // Empty section_id on Replace is omitted just like None.
    let replace_empty_sec = CanvasEditOp::Replace {
        section_id: Some(String::new()),
        markdown: "whole".into(),
    }
    .to_change_json();
    assert!(replace_empty_sec.get("section_id").is_none());

    let delete = CanvasEditOp::Delete {
        section_id: "S3".into(),
    }
    .to_change_json();
    assert_eq!(delete, json!({ "operation": "delete", "section_id": "S3" }));
}

#[test]
fn canvas_access_level_meta_errors_and_comments_parsers() {
    use std::collections::HashMap;

    // CanvasAccessLevel
    assert_eq!(CanvasAccessLevel::Read.as_str(), "read");
    assert_eq!(CanvasAccessLevel::Write.as_str(), "write");
    assert_eq!(CanvasAccessLevel::Owner.as_str(), "owner");
    assert_eq!(
        CanvasAccessLevel::parse("read"),
        Some(CanvasAccessLevel::Read)
    );
    assert_eq!(
        CanvasAccessLevel::parse(" WRITE "),
        Some(CanvasAccessLevel::Write)
    );
    assert_eq!(
        CanvasAccessLevel::parse("Owner"),
        Some(CanvasAccessLevel::Owner)
    );
    assert_eq!(CanvasAccessLevel::parse("comment"), None);
    assert_eq!(CanvasAccessLevel::parse(""), None);

    // canvas_comment_channel_id
    assert_eq!(
        canvas_comment_channel_id("F0123ABC"),
        ChannelId::from("C0123ABC")
    );
    assert_eq!(
        canvas_comment_channel_id("C0123ABC"),
        ChannelId::from("C0123ABC")
    );
    assert_eq!(canvas_comment_channel_id(""), ChannelId::from(""));

    // parse_canvas_meta_from_file: full fields from File.extra
    let rich_file: crate::types::File = serde_json::from_value(json!({
        "id": "F099CANVAS",
        "title": "Q4 Roadmap",
        "permalink": "https://acme.slack.com/docs/T1/F099CANVAS",
        "user": "U0CREATOR_FALLBACK",
        "editable": true,
        "access": "owner",
        "org_or_workspace_access": "write",
        "canvas_printing_enabled": false,
        "canvas_creator_id": "U0OWNER",
        "editors": ["U0OWNER", "U0EDITOR2", "", 42],
        "quip_thread_id": "OYP123XYZ"
    }))
    .expect("canvas file parses");
    let meta = parse_canvas_meta_from_file(&rich_file, None);
    assert_eq!(meta.file_id, "F099CANVAS");
    assert_eq!(meta.title, "Q4 Roadmap");
    assert_eq!(
        meta.permalink.as_deref(),
        Some("https://acme.slack.com/docs/T1/F099CANVAS")
    );
    assert!(meta.editable);
    assert_eq!(meta.access.as_deref(), Some("owner"));
    assert_eq!(meta.org_or_workspace_access.as_deref(), Some("write"));
    assert!(!meta.canvas_printing_enabled);
    assert_eq!(
        meta.canvas_creator_id,
        Some(crate::types::UserId::from("U0OWNER"))
    );
    assert_eq!(
        meta.editors,
        vec![
            crate::types::UserId::from("U0OWNER"),
            crate::types::UserId::from("U0EDITOR2")
        ]
    );
    assert_eq!(meta.quip_thread_id.as_deref(), Some("OYP123XYZ"));
    assert_eq!(meta.comment_channel_id, ChannelId::from("C099CANVAS"));

    // parse_canvas_meta_from_file: defaults + raw fallback
    let minimal_file: crate::types::File =
        serde_json::from_value(json!({ "id": "F0MIN", "name": "Fallback Name", "user": "U1" }))
            .expect("minimal canvas file parses");
    let raw_override = json!({
        "file": {
            "editable": "true",
            "access": "read"
        }
    });
    let min_meta = parse_canvas_meta_from_file(&minimal_file, Some(&raw_override));
    assert_eq!(min_meta.title, "Fallback Name");
    assert!(min_meta.editable);
    assert_eq!(min_meta.access.as_deref(), Some("read"));
    assert!(
        min_meta.canvas_printing_enabled,
        "canvas_printing_enabled must default to true when absent"
    );
    assert_eq!(
        min_meta.canvas_creator_id,
        Some(crate::types::UserId::from("U1"))
    );
    assert_eq!(min_meta.comment_channel_id, ChannelId::from("C0MIN"));

    // Empty file defaults title to "Canvas" and editable to false.
    let blank_meta = parse_canvas_meta_from_file(&crate::types::File::default(), None);
    assert_eq!(blank_meta.title, "Canvas");
    assert!(!blank_meta.editable);
    assert!(blank_meta.canvas_printing_enabled);

    // format_canvas_error: every documented error code + prefixed + unknown passthrough
    for code in [
        "free_teams_cannot_create_standalone_canvases",
        "free_team_canvas_tab_already_exists",
        "channel_canvas_already_exists",
        "restricted_action",
        "canvas_printing_disabled",
        "not_allowed_token_type",
        "canvas_not_found",
        "invalid_parameters",
        "missing_scope",
        "cant_delete_file",
    ] {
        let translated = format_canvas_error(code);
        assert_ne!(translated, code, "expected friendly message for {code}");
        assert_eq!(
            format_canvas_error(&format!("slack api error: {code}")),
            translated
        );
    }
    assert_eq!(
        format_canvas_error("some_custom_unmapped_error"),
        "some_custom_unmapped_error"
    );

    // parse_canvas_comment_threads: roots, replies, skipping thread replies in history,
    // and all resolved flag representations.
    let history: Vec<crate::types::Message> = serde_json::from_value(json!([
        {
            "ts": "1700.1",
            "user": "U1",
            "text": "Open thread",
            "subtype": "document_comment_root",
            "reply_count": 1,
            "document_comment": { "is_archived": false }
        },
        {
            "ts": "1700.15",
            "thread_ts": "1700.1",
            "user": "U2",
            "text": "Inline reply in history should not become a root"
        },
        {
            "ts": "1700.2",
            "user": "U2",
            "text": "Resolved via is_archived",
            "is_archived": true
        },
        {
            "ts": "1700.3",
            "text": "Resolved via quip_thread_archived",
            "quip_thread_archived": "true",
            "document_comment": { "authors": ["U3"] }
        },
        {
            "ts": "1700.4",
            "user": "U4",
            "text": "Resolved via document_comment.is_archived",
            "document_comment": { "is_archived": true }
        },
        {
            "ts": "1700.5",
            "user": "U5",
            "text": "Resolved via subtype",
            "subtype": "document_comment_archived"
        }
    ]))
    .expect("comment history parses");

    let thread_replies: Vec<crate::types::Message> = serde_json::from_value(json!([
        { "ts": "1700.1", "user": "U1", "text": "Open thread" },
        { "ts": "1700.15", "thread_ts": "1700.1", "user": "U2", "text": "First reply" },
        { "ts": "1700.16", "thread_ts": "1700.1", "user": "U3", "text": "Second reply" }
    ]))
    .expect("replies parse");

    let mut replies_map = HashMap::new();
    replies_map.insert(Ts::from("1700.1"), thread_replies);

    let threads = parse_canvas_comment_threads(&history, &replies_map);
    assert_eq!(threads.len(), 5);

    assert_eq!(threads[0].root_ts, Ts::from("1700.1"));
    assert_eq!(threads[0].author, Some(crate::types::UserId::from("U1")));
    assert!(!threads[0].resolved);
    assert_eq!(threads[0].reply_count, 2);
    assert_eq!(threads[0].replies.len(), 2);
    assert_eq!(threads[0].replies[0].text, "First reply");
    assert_eq!(threads[0].replies[1].text, "Second reply");

    assert!(threads[1].resolved);
    assert!(threads[2].resolved);
    assert_eq!(threads[2].author, Some(crate::types::UserId::from("U3")));
    assert!(threads[3].resolved);
    assert!(threads[4].resolved);
}

#[test]
fn lists_schema_records_cells_and_inputs_handle_missing_null_and_drifted_shapes() {
    // Fallback title -> name -> "List", missing metadata, empty id/key skipped, id <- key, choice label <- value.
    let (t1, d1, c1) = parse_list_schema(&json!({}));
    assert_eq!((t1.as_str(), d1.as_str(), c1.len()), ("List", "", 0));

    let (t2, _d2, c2) = parse_list_schema(&json!({
        "title": "",
        "name": "Fallback Name",
        "list_metadata": {
            "description": null,
            "schema": [
                { "id": "", "key": "" },
                { "key": "owner", "type": "user" },
                {
                    "id": "ColSel",
                    "name": "",
                    "options": {
                        "choices": [
                            { "value": "opt_a" },
                            { "value": "", "label": "" }
                        ]
                    }
                }
            ]
        }
    }));
    assert_eq!(t2, "Fallback Name");
    assert_eq!(c2.len(), 2);
    assert_eq!(c2[0].id, "owner");
    assert_eq!(c2[0].name, "owner");
    assert_eq!(c2[1].id, "ColSel");
    assert_eq!(c2[1].name, "ColSel");
    assert_eq!(c2[1].kind, "text");
    assert_eq!(c2[1].choices.len(), 1);
    assert_eq!(c2[1].choices[0].label, "opt_a");

    // Drifted / null / empty-array cells never panic and return Empty when appropriate.
    for empty_field in [
        json!(null),
        json!({ "checkbox": [] }),
        json!({ "checkbox": ["not-bool"] }),
        json!({ "user": ["", null, 123] }),
        json!({ "select": ["", 42] }),
        json!({ "date": [""] }),
        json!({ "link": [{}] }),
        json!({ "number": [true] }),
        json!({ "text": "   \t\n  " }),
        json!({ "rich_text": [null, 42, { "elements": "bad" }] }),
        json!({ "email": [123, null] }),
        json!({ "value": "   " }),
    ] {
        assert_eq!(
            parse_list_cell(&empty_field),
            SlackListCellValue::Empty,
            "expected Empty for {empty_field}"
        );
    }

    // Record stores cell once under column_id (falling back to key when column_id is empty).
    let rows = parse_list_records(&[json!({
        "id": "R1",
        "fields": [
            { "column_id": "", "key": "only_key", "text": "alpha" },
            { "column_id": "Col2", "key": "second_key", "number": [7.0] },
            { "column_id": "ColBig", "number": [1e25] },
            { "column_id": "", "key": "", "text": "ignored" },
            { "column_id": "ColEmpty", "text": "   " }
        ]
    })]);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].cells.len(), 3);
    assert_eq!(
        rows[0].cells.get("only_key"),
        Some(&SlackListCellValue::Text("alpha".into()))
    );
    assert_eq!(
        rows[0].cells.get("Col2"),
        Some(&SlackListCellValue::Number("7".into()))
    );
    assert!(
        !rows[0].cells.contains_key("second_key"),
        "must not double-insert under both column_id and key"
    );
    assert_eq!(
        rows[0].cell(&SlackListColumn {
            id: "Col2".into(),
            key: "second_key".into(),
            ..Default::default()
        }),
        Some(&SlackListCellValue::Number("7".into()))
    );
    assert_eq!(
        rows[0].cells.get("ColBig"),
        Some(&SlackListCellValue::Number(1e25_f64.to_string()))
    );
    assert!(!rows[0].cells.contains_key("ColEmpty"));

    // ListCreateWire / ListRecordCreateWire empty payloads.
    let lc: ListCreateWire = serde_json::from_str("{}").expect("empty list create");
    assert!(lc.list.is_none() && lc.list_id.is_none());
    let lrc: ListRecordCreateWire = serde_json::from_str("{}").expect("empty record create");
    assert!(lrc.record.is_none() && lrc.item.is_none());

    // ListCellInput http:// URL detection and empty display_name.
    let http_cell = ListCellInput {
        column_id: "C1".into(),
        value: ListCellInputValue::Text("http://internal.test".into()),
    }
    .to_json(None);
    assert_eq!(
        http_cell["rich_text"][0]["elements"][0]["elements"][0]["type"],
        "link"
    );
    let empty_name_link = ListCellInput {
        column_id: "C2".into(),
        value: ListCellInputValue::Link {
            url: "https://example.test".into(),
            display_name: Some(String::new()),
        },
    }
    .to_json(None);
    assert_eq!(empty_name_link["link"][0]["display_as_url"], true);
    assert!(empty_name_link["link"][0].get("display_name").is_none());
}

#[test]
fn huddles_and_calls_wire_handle_missing_null_and_partial_payloads() {
    let empty: RoomsJoinWire = serde_json::from_str("{}").expect("empty rooms.join");
    assert!(empty.call.is_none());

    let null_call: RoomsJoinWire =
        serde_json::from_value(json!({ "call": null })).expect("null call");
    assert!(null_call.call.is_none());

    let partial: RoomsJoinWire = serde_json::from_value(json!({
        "call": {
            "call_id": "R1",
            "free_willy": {
                "attendee": { "JoinToken": "tok" },
                "meeting": { "MediaPlacement": { "SignalingUrl": "wss://sig.test" } }
            },
            "huddle": {},
            "canvas": { "file_id": "F_NOTES" }
        }
    }))
    .expect("partial rooms.join");
    let call = partial.call.expect("call present");
    let chime = call.free_willy.expect("free_willy present");
    assert_eq!(chime.attendee.join_token, "tok");
    assert_eq!(chime.attendee.capabilities, ChimeCapabilities::default());
    assert_eq!(
        chime.meeting.media_placement.signaling_url,
        "wss://sig.test"
    );
    assert_eq!(chime.meeting.media_placement.audio_host_url, "");
    assert_eq!(
        call.canvas.and_then(|c| c.file_id),
        Some("F_NOTES".to_owned())
    );

    let link_a: RoomsGetLinkWire =
        serde_json::from_value(json!({ "huddle_link": "https://app.slack.com/huddle/T1/C1" }))
            .expect("huddle_link");
    assert_eq!(link_a.link, None);
    assert_eq!(
        link_a.huddle_link.as_deref(),
        Some("https://app.slack.com/huddle/T1/C1")
    );
    let link_empty: RoomsGetLinkWire = serde_json::from_str("{}").expect("empty link");
    assert!(
        link_empty.link.is_none() && link_empty.huddle_link.is_none() && link_empty.url.is_none()
    );
}

#[test]
fn users_usergroups_search_and_edge_search_tolerate_drift() {
    // SearchModulesMessages skips empty channel id, normalizes empty name -> None, clamps page 0 -> 1.
    let mod_msgs: SearchModulesMessagesWire = serde_json::from_value(json!({
        "pagination": { "total_count": 1, "page": 0, "page_count": 1 },
        "items": [
            { "channel": { "id": "", "name": "ghost" }, "messages": [{ "ts": "1.0", "text": "skip" }] },
            {
                "channel": { "id": "C2", "name": "" },
                "messages": [
                    { "ts": "2.0", "text": "\u{e000}hit\u{e001}" },
                    "corrupt-msg"
                ]
            }
        ]
    }))
    .expect("search.modules.messages parses");
    let page = parse_search_modules_messages(mod_msgs);
    assert_eq!(page.page, 1);
    assert_eq!(page.hits.len(), 1);
    assert_eq!(page.hits[0].channel_name, None);
    assert_eq!(page.hits[0].message.text, "hit");

    let mod_files: SearchModulesFilesWire = serde_json::from_value(json!({
        "pagination": { "total_count": 2, "page": 0, "page_count": 1 },
        "items": [{ "id": "F1" }, "corrupt-file"]
    }))
    .expect("search.modules.files parses");
    let fpage = parse_search_modules_files(mod_files);
    assert_eq!(fpage.page, 1);
    assert_eq!(fpage.files.len(), 1);

    // EdgeUsersWire / EdgeChannelsWire with corrupt entries & null member_channels.
    let edge_u: EdgeUsersWire = serde_json::from_value(json!({
        "results": [{ "id": "U1", "name": "ana" }, 123]
    }))
    .expect("edge users parse");
    assert_eq!(edge_u.results.len(), 1);

    let edge_c: EdgeChannelsWire = serde_json::from_value(json!({
        "results": [{ "id": "C1", "name": "eng" }, null],
        "member_channels": null
    }))
    .expect("edge channels parse");
    assert_eq!(edge_c.results.len(), 1);
    assert!(edge_c.member_channels.is_empty());

    // Sparse ProfileUpdate omits all None fields.
    assert_eq!(
        serde_json::to_value(ProfileUpdate::default()).expect("serialize empty ProfileUpdate"),
        json!({})
    );

    // Usergroup deserialization tolerance (missing/null/extra fields).
    let ug: Usergroup = serde_json::from_value(json!({
        "id": "S01",
        "name": null,
        "handle": "oncall",
        "description": null,
        "user_count": 3,
        "date_delete": null,
        "unknown_ug_field": "kept"
    }))
    .expect("drifted usergroup parses");
    assert_eq!(ug.id.as_str(), "S01");
    assert_eq!(ug.name, "");
    assert_eq!(ug.handle, "oncall");
    assert_eq!(ug.user_count, Some(3));
    assert!(!ug.is_deleted());
    assert_eq!(ug.extra.get("unknown_ug_field"), Some(&json!("kept")));
}

#[test]
fn apps_home_commands_and_action_container_edge_cases() {
    // AppHomeWire with null app_home, fallback to app_home.view, and implied home_tab_enabled.
    let empty_home: AppHomeWire =
        serde_json::from_value(json!({ "app_home": null })).expect("null app_home");
    let resp = empty_home.into_response();
    assert!(!resp.home_tab_enabled && !resp.messages_tab_enabled && resp.home_view.is_none());

    let nested_view: AppHomeWire = serde_json::from_value(json!({
        "app_home": {
            "home_tab_enabled": false,
            "messages_tab_enabled": true,
            "view": { "id": "V_NESTED" }
        }
    }))
    .expect("nested app_home.view");
    let resp = nested_view.into_response();
    assert!(resp.home_tab_enabled, "home_view presence enables home tab");
    assert!(resp.messages_tab_enabled);
    assert_eq!(
        resp.home_view.as_ref().and_then(|v| v["id"].as_str()),
        Some("V_NESTED")
    );

    // ActionContainer message vs attachment vs view precedence.
    let msg_c = ActionContainer {
        channel: ChannelId::from("C1"),
        message_ts: Ts::from("1.1"),
        attachment_id: None,
        is_ephemeral: true,
        view_id: None,
    };
    assert_eq!(
        msg_c.json(),
        json!({
            "type": "message",
            "message_ts": "1.1",
            "channel_id": "C1",
            "is_ephemeral": true
        })
    );
    let att_c = ActionContainer {
        attachment_id: Some(7),
        ..msg_c.clone()
    };
    assert_eq!(att_c.json()["type"], "message_attachment");
    assert_eq!(att_c.json()["attachment_id"], 7);

    // parse_app_commands: case-insensitive dedup, blank skip, empty app_name -> service_name, empty desc.
    let cmds_wire: AppCommandsWire = serde_json::from_value(json!({
        "commands": [
            { "name": "  /Jira ", "app": "", "app_name": "", "service_name": "JiraSvc", "desc": "  File issue " },
            { "name": "/JIRA", "desc": "duplicate ignored" },
            { "name": "///", "desc": "blank ignored" },
            { "name": "bare", "desc": "   ", "alias_of": "" },
            "corrupt-cmd"
        ]
    }))
    .expect("app commands parse");
    let cmds = parse_app_commands(cmds_wire);
    assert_eq!(cmds.len(), 2);
    assert_eq!(cmds[0].name, "jira");
    assert_eq!(cmds[0].app_name.as_deref(), Some("JiraSvc"));
    assert_eq!(cmds[0].app_id, None);
    assert_eq!(cmds[0].description, "File issue");
    assert_eq!(cmds[1].name, "bare");
    assert_eq!(cmds[1].description, "");

    // page() helper extracts next_cursor from ApiResponse.
    let api_resp: ApiResponse<FilesWire> = ApiResponse::from_value(json!({
        "ok": true,
        "files": [{ "id": "F1" }],
        "response_metadata": { "next_cursor": "next_123" }
    }))
    .expect("api response parses");
    let p = page(api_resp, |w| w.files);
    assert_eq!(p.items.len(), 1);
    assert_eq!(p.next_cursor.as_deref(), Some("next_123"));
    assert!(client_token().starts_with("web-"));
}
