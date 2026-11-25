use std::sync::{Arc, Mutex};

use gpui_kit::gpui::{self, TestAppContext};
use slackless_core::SharedStore;
use slackless_core::api::{
    RemoteFilePage, RemoteMessageHit, RemoteMessagePage, SearchSort as ApiSearchSort,
};
use slackless_core::store::{SearchFilters, Store};
use slackless_core::types::{
    ChannelId, Conversation, File, Message, Team, TeamId, TopicPurpose, Ts, UserId,
};

use super::SearchView;
use super::state::SearchTab;

#[gpui::test]
fn search_view_local_fts_exclusions_remote_query_pagination_and_channel_dedup(
    cx: &mut TestAppContext,
) {
    let team_id = TeamId::from("T0SEARCH");
    let ch_eng = Conversation {
        id: ChannelId::from("C0ENG"),
        name: Some("engineering".into()),
        is_channel: true,
        topic: Some(TopicPurpose {
            value: "Release deployments".into(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let ch_noise = Conversation {
        id: ChannelId::from("C0NOISE"),
        name: Some("alerts-bot".into()),
        is_channel: true,
        ..Default::default()
    };
    let ch_dir_only = Conversation {
        id: ChannelId::from("C0OPS"),
        name: Some("ops-deployments".into()),
        is_channel: true,
        ..Default::default()
    };
    let dm = Conversation {
        id: ChannelId::from("D0ALICE"),
        is_im: true,
        user: Some(UserId::from("U0ALICE")),
        ..Default::default()
    };

    let mut store = Store::open_in_memory().expect("in-memory store");
    for conv in [&ch_eng, &ch_noise, &dm] {
        store.upsert_conversation(&team_id, conv).unwrap();
    }
    store
        .upsert_message(
            &team_id,
            &ch_eng.id,
            &Message {
                ts: Ts::from("1700000001.000100"),
                user: Some(UserId::from("U0ALICE")),
                text: "canary deployment looks healthy".into(),
                ..Default::default()
            },
        )
        .unwrap();
    store
        .upsert_message(
            &team_id,
            &ch_noise.id,
            &Message {
                ts: Ts::from("1700000002.000100"),
                user: Some(UserId::from("U0BOT")),
                text: "canary alert fired".into(),
                ..Default::default()
            },
        )
        .unwrap();
    store
        .insert_huddle_transcript_segment(
            &team_id,
            &slackless_core::store::NewHuddleTranscriptSegment {
                channel: &ch_eng.id,
                call_id: "R0HUDDLE1",
                thread_root_ts: None,
                speaker_id: Some(&UserId::from("U0ALICE")),
                lang: Some("sv"),
                source: "parakeet-v3",
                text: "Vi tar ett snabbt möte om nya releasen efter lunch.",
                at_ms: 1_700_000_100_000,
            },
        )
        .unwrap();
    let archive: SharedStore = Arc::new(Mutex::new(store));

    let mut fx = crate::fixtures::load();
    fx.sample = false;
    fx.workspaces = vec![Team {
        id: team_id,
        name: "Acme".into(),
        ..Default::default()
    }];
    fx.channels = vec![ch_eng.clone(), ch_noise.clone()];
    fx.directory = vec![ch_eng, ch_dir_only, dm.clone()];
    fx.dms = vec![dm];

    let view = cx.update(|cx| SearchView::new(Arc::new(fx), Some(archive), cx));

    // 1. Local search + excluded_channels applied only when `in_channels` is empty.
    view.update(cx, |v, cx| {
        v.set_excluded_channels("#alerts-bot,  ");
        v.run("canary".into(), cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert_eq!(v.local_results.len(), 1);
        assert_eq!(v.local_results[0].channel, ChannelId::from("C0ENG"));
    });

    // Explicit `in_channels` overrides default excluded_channels.
    view.update(cx, |v, cx| {
        let filters = SearchFilters {
            in_channels: vec!["alerts-bot".into()],
            ..Default::default()
        };
        v.run_with_filters("canary".into(), filters, cx);
    });
    cx.run_until_parked();
    view.update(cx, |v, cx| {
        assert_eq!(v.local_results.len(), 1);
        assert_eq!(v.local_results[0].channel, ChannelId::from("C0NOISE"));

        // 2. remote_query_string formatting with IDs vs names, with:, and is:thread.
        let rich_filters = SearchFilters {
            in_channels: vec!["C0ENG".into(), "general".into()],
            from_users: vec!["U0ALICE".into(), "bob".into()],
            includes_user: Some("U0BOB".into()),
            threads_only: true,
            ..Default::default()
        };
        v.run_with_filters("deploy".into(), rich_filters, cx);
        assert_eq!(
            v.remote_query_string(),
            "deploy in:<#C0ENG> in:#general from:<@U0ALICE> from:@bob with:<@U0BOB> is:thread"
        );

        // 3. set_remote_messages: stale query/sort rejected; page 1 replaces; page 2 appends; Err sets error.
        v.run("deploy".into(), cx);
        let make_hit = |ts: &str| RemoteMessageHit {
            channel: ChannelId::from("C0ENG"),
            channel_name: Some("engineering".into()),
            message: Message {
                ts: Ts::from(ts),
                user: Some(UserId::from("U0ALICE")),
                text: "deploy ok".into(),
                ..Default::default()
            },
        };
        // Stale query ignored
        v.set_remote_messages(
            "other-query",
            ApiSearchSort::Timestamp,
            1,
            Ok(RemoteMessagePage {
                total: 1,
                page: 1,
                page_count: 1,
                hits: vec![make_hit("1.000001")],
            }),
            cx,
        );
        assert!(v.remote_messages.is_empty());

        // Stale sort ignored
        v.set_remote_messages(
            "deploy",
            ApiSearchSort::Score,
            1,
            Ok(RemoteMessagePage {
                total: 1,
                page: 1,
                page_count: 1,
                hits: vec![make_hit("1.000001")],
            }),
            cx,
        );
        assert!(v.remote_messages.is_empty());

        // Page 1 accepted, page 2 appended
        v.set_remote_messages(
            "deploy",
            ApiSearchSort::Timestamp,
            1,
            Ok(RemoteMessagePage {
                total: 2,
                page: 1,
                page_count: 2,
                hits: vec![make_hit("1.000001")],
            }),
            cx,
        );
        assert_eq!(v.remote_messages.len(), 1);
        assert_eq!(v.remote_messages_total, 2);

        v.set_remote_messages(
            "deploy",
            ApiSearchSort::Timestamp,
            2,
            Ok(RemoteMessagePage {
                total: 2,
                page: 2,
                page_count: 2,
                hits: vec![make_hit("1.000002")],
            }),
            cx,
        );
        assert_eq!(v.remote_messages.len(), 2);
        assert_eq!(v.remote_messages_page, 2);

        // set_remote_files: stale rejected, page 1 + page 2 append, Err sets error.
        let make_file = |id: &str| File {
            id: slackless_core::types::FileId::from(id),
            name: Some(format!("{id}.pdf")),
            ..Default::default()
        };
        v.set_remote_files(
            "stale",
            ApiSearchSort::Timestamp,
            1,
            Ok(RemoteFilePage {
                total: 1,
                page: 1,
                page_count: 1,
                files: vec![make_file("F0")],
            }),
            cx,
        );
        assert!(v.remote_files.is_empty());

        v.set_remote_files(
            "deploy",
            ApiSearchSort::Timestamp,
            1,
            Ok(RemoteFilePage {
                total: 2,
                page: 1,
                page_count: 2,
                files: vec![make_file("F1")],
            }),
            cx,
        );
        v.set_remote_files(
            "deploy",
            ApiSearchSort::Timestamp,
            2,
            Ok(RemoteFilePage {
                total: 2,
                page: 2,
                page_count: 2,
                files: vec![make_file("F2")],
            }),
            cx,
        );
        assert_eq!(v.remote_files.len(), 2);

        v.set_remote_files(
            "deploy",
            ApiSearchSort::Timestamp,
            3,
            Err("rate limited".into()),
            cx,
        );
        assert_eq!(v.remote_files_error.as_deref(), Some("rate limited"));

        // 4. matching_channels deduplicates C0ENG across channels+directory, matches topic, excludes DMs.
        let matched_ids: Vec<&str> = v
            .matching_channels()
            .into_iter()
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(matched_ids, vec!["C0ENG", "C0OPS"]);

        // 5. Huddle transcripts tab searches local SQLite FTS5 with Swedish åäö diacritic folding.
        v.set_tab(SearchTab::Transcripts, cx);
        v.run("mote".into(), cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert_eq!(v.transcript_results.len(), 1);
        assert_eq!(v.transcript_results[0].call_id, "R0HUDDLE1");
        assert_eq!(v.transcript_results[0].lang.as_deref(), Some("sv"));
    });
}
