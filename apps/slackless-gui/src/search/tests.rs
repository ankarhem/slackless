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
use super::query::transcript_match_excerpt;
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
    let team = Team {
        id: team_id,
        name: "Acme".into(),
        ..Default::default()
    };
    fx.teams = vec![team.clone()];
    fx.set_rail_workspaces(vec![team]);
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
        assert_eq!(v.transcript_results[0].segment.call_id, "R0HUDDLE1");
        assert_eq!(v.transcript_results[0].segment.lang.as_deref(), Some("sv"));
    });
}

#[gpui::test]
fn search_view_all_workspaces_scope_merges_hits_and_snippets_across_stores(
    cx: &mut TestAppContext,
) {
    use super::state::{SearchScope, SearchWorkspaceSource};
    use slackless_core::search::SearchHitKind;
    use slackless_core::types::{FileId, FileMode};

    let ws_a = TeamId::from("T_A");
    let ws_b = TeamId::from("T_B");

    let mut store_a = Store::open_in_memory().expect("store_a");
    store_a
        .upsert_team(
            &ws_a,
            &Team {
                id: ws_a.clone(),
                name: "Alpha".into(),
                ..Default::default()
            },
        )
        .unwrap();
    store_a
        .upsert_conversation(
            &ws_a,
            &Conversation {
                id: ChannelId::from("C_A1"),
                name: Some("ops".into()),
                is_channel: true,
                ..Default::default()
            },
        )
        .unwrap();
    store_a
        .upsert_message(
            &ws_a,
            &ChannelId::from("C_A1"),
            &Message {
                ts: Ts::from("1700000100.000000"),
                text: "Deploying hyperion cluster".into(),
                ..Default::default()
            },
        )
        .unwrap();

    let mut store_b = Store::open_in_memory().expect("store_b");
    store_b
        .upsert_team(
            &ws_b,
            &Team {
                id: ws_b.clone(),
                name: "Beta".into(),
                ..Default::default()
            },
        )
        .unwrap();
    store_b
        .upsert_conversation(
            &ws_b,
            &Conversation {
                id: ChannelId::from("C_B1"),
                name: Some("platform".into()),
                is_channel: true,
                ..Default::default()
            },
        )
        .unwrap();
    store_b
        .upsert_message(
            &ws_b,
            &ChannelId::from("C_B1"),
            &Message {
                ts: Ts::from("1700000200.000000"),
                text: "Attached config".into(),
                files: vec![File {
                    id: FileId::from("F_SNIP"),
                    name: Some("hyperion.toml".into()),
                    mode: Some(FileMode::Snippet),
                    preview: Some("cluster = \"hyperion\"".into()),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .unwrap();

    let shared_a: SharedStore = Arc::new(Mutex::new(store_a));
    let shared_b: SharedStore = Arc::new(Mutex::new(store_b));

    let mut fx = crate::fixtures::load();
    fx.sample = true;
    let teams = vec![
        Team {
            id: ws_a.clone(),
            name: "Alpha".into(),
            ..Default::default()
        },
        Team {
            id: ws_b.clone(),
            name: "Beta".into(),
            ..Default::default()
        },
    ];
    fx.teams = teams.clone();
    fx.set_rail_workspaces(teams);

    let view = cx.update(|cx| SearchView::new(Arc::new(fx), Some(shared_a.clone()), cx));
    view.update(cx, |v, cx| {
        v.set_workspace_sources(
            vec![
                SearchWorkspaceSource {
                    team_id: ws_a.clone(),
                    name: "Alpha".into(),
                    store: shared_a.clone(),
                },
                SearchWorkspaceSource {
                    team_id: ws_b.clone(),
                    name: "Beta".into(),
                    store: shared_b.clone(),
                },
            ],
            cx,
        );
        v.run("hyperion".into(), cx);
    });
    cx.run_until_parked();
    // CurrentWorkspace scope returns only Alpha's hit.
    view.read_with(cx, |v, _| {
        assert_eq!(v.local_results.len(), 1);
        assert_eq!(v.local_results[0].workspace, ws_a);
    });

    // Switching to AllWorkspaces merges Beta's snippet + Alpha's message, newest first.
    view.update(cx, |v, cx| v.set_scope(SearchScope::AllWorkspaces, cx));
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert_eq!(v.local_results.len(), 2);
        assert_eq!(v.local_results[0].workspace, ws_b);
        assert_eq!(v.local_results[0].workspace_name.as_deref(), Some("Beta"));
        assert_eq!(v.local_results[0].kind, SearchHitKind::Snippet);
        assert_eq!(v.local_results[1].workspace, ws_a);
        assert_eq!(v.local_results[1].kind, SearchHitKind::Message);
    });
}

#[test]
fn transcript_match_excerpt_handles_case_word_boundaries_modifiers_and_non_matches() {
    // Short transcript matching case-insensitively returns full trimmed text without ellipses.
    let short = "We shipped the Checkout rollout yesterday afternoon.";
    assert_eq!(
        transcript_match_excerpt(short, "CHECKOUT").as_deref(),
        Some(short)
    );

    // Search modifiers are stripped before matching free-text terms.
    assert_eq!(
        transcript_match_excerpt(short, "in:#general from:@alice rollout").as_deref(),
        Some(short)
    );

    // No match or empty/modifier-only query returns None.
    assert_eq!(transcript_match_excerpt(short, "kubernetes"), None);
    assert_eq!(transcript_match_excerpt(short, ""), None);
    assert_eq!(
        transcript_match_excerpt(short, "in:#general has:file"),
        None
    );

    // Long transcript windows cleanly around the first matching term on word boundaries.
    let long = "Welcome everyone to the weekly engineering sync. First we reviewed Q4 latency numbers across all payment adapters and confirmed p99 stayed below forty milliseconds. Next we discussed the upcoming canary deployment schedule for Stockholm and Oslo regions before wrapping up with action items for Friday.";
    let excerpt = transcript_match_excerpt(long, "canary").expect("must match canary");
    assert!(
        excerpt.starts_with('\u{2026}'),
        "expected leading ellipsis: {excerpt}"
    );
    assert!(
        excerpt.ends_with('\u{2026}'),
        "expected trailing ellipsis: {excerpt}"
    );
    assert!(excerpt.contains("canary deployment"));
    assert!(
        excerpt.chars().count() <= 130,
        "expected ~120-char window, got {} chars: {excerpt}",
        excerpt.chars().count()
    );
    // Word-boundary snap: shouldn't cut "upcoming" or "canary" mid-word.
    let inner = excerpt.trim_matches('\u{2026}');
    assert!(!inner.starts_with(' '), "trimmed inner start: {excerpt}");
    assert!(!inner.ends_with(' '), "trimmed inner end: {excerpt}");

    // Non-ASCII / Swedish text does not panic on Unicode char boundaries.
    let sv = "Hej alla, vi kör en snabb genomgång av ÅÄÖ-releasen för betalflödet imorgon bitti innan kunderna vaknar och testar hela flödet i produktion tillsammans.";
    let sv_excerpt = transcript_match_excerpt(sv, "betalflödet").expect("must match swedish term");
    assert!(sv_excerpt.contains("betalflödet"));
}

#[gpui::test]
fn search_view_keyword_hybrid_semantic_modes_and_missing_model_prompt(cx: &mut TestAppContext) {
    use slackless_core::embeddings::{Embedder, HashProjectionEmbedder};
    use slackless_core::semantic_index::index_store_documents;
    use slackless_core::store::{SearchMode, SemanticIndex};

    let ws = TeamId::from("T_SEM");
    let ch = Conversation {
        id: ChannelId::from("C_INFRA"),
        name: Some("infra".into()),
        is_channel: true,
        ..Default::default()
    };
    let mut store = Store::open_in_memory().expect("store");
    store
        .upsert_team(
            &ws,
            &Team {
                id: ws.clone(),
                name: "SemanticCorp".into(),
                ..Default::default()
            },
        )
        .unwrap();
    store.upsert_conversation(&ws, &ch).unwrap();
    store
        .upsert_messages(
            &ws,
            &ch.id,
            &[
                Message {
                    ts: Ts::from("1700001000.000000"),
                    user: Some(UserId::from("U1")),
                    text: "Kubernetes cluster autoscaling deployment pipeline".into(),
                    ..Default::default()
                },
                Message {
                    ts: Ts::from("1700001005.000000"),
                    user: Some(UserId::from("U1")),
                    text: "Lunch salad bar menu today".into(),
                    ..Default::default()
                },
            ],
        )
        .unwrap();

    let embedder: Arc<dyn Embedder> = Arc::new(HashProjectionEmbedder::new(128));
    let mut sem_idx = SemanticIndex::open_in_memory().unwrap();
    index_store_documents(&store, &mut sem_idx, embedder.as_ref(), Some(&ws), |_| {}).unwrap();
    let shared_idx = Arc::new(Mutex::new(sem_idx));
    let archive: SharedStore = Arc::new(Mutex::new(store));

    let mut fx = crate::fixtures::load();
    fx.sample = true;
    let team = Team {
        id: ws,
        name: "SemanticCorp".into(),
        ..Default::default()
    };
    fx.teams = vec![team.clone()];
    fx.set_rail_workspaces(vec![team]);
    fx.channels = vec![ch];

    let view = cx.update(|cx| SearchView::new(Arc::new(fx), Some(archive), cx));

    // Default mode is Keyword; switching to Semantic sets sort to Relevant and finds semantic matches.
    view.update(cx, |v, cx| {
        assert_eq!(v.mode(), SearchMode::Keyword);
        v.set_semantic_backend_for_test(Arc::clone(&embedder), Arc::clone(&shared_idx));
        v.set_mode(SearchMode::Semantic, cx);
        assert_eq!(v.sort, slackless_core::store::SearchSort::Relevant);
        v.run("kubernetes autoscaler deploying".into(), cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert_eq!(v.mode(), SearchMode::Semantic);
        assert!(!v.semantic_model_missing_for_test());
        assert!(
            !v.local_hits_for_test().is_empty(),
            "expected semantic hit for morphologically related query"
        );
        assert!(
            v.local_hits_for_test()[0]
                .message
                .text
                .contains("Kubernetes")
        );
        assert!(v.local_hits_for_test()[0].semantic_score.is_some());
    });

    // Explicit `Most recent` toggle is honored in Semantic mode (not silently overridden).
    view.update(cx, |v, cx| {
        v.set_sort(slackless_core::store::SearchSort::Recent, cx);
        assert_eq!(v.sort, slackless_core::store::SearchSort::Recent);
    });
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert_eq!(v.sort, slackless_core::store::SearchSort::Recent);
        assert!(!v.local_hits_for_test().is_empty());
    });

    // Hybrid mode blends FTS5 + semantic via RRF.
    view.update(cx, |v, cx| {
        v.set_mode(SearchMode::Hybrid, cx);
        v.run("kubernetes deployment".into(), cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert_eq!(v.mode(), SearchMode::Hybrid);
        assert_eq!(v.local_hits_for_test().len(), 1);
        assert!(
            v.local_hits_for_test()[0]
                .message
                .text
                .contains("Kubernetes")
        );
    });

    // Switching back to Keyword keeps standard FTS5 behavior cleanly.
    view.update(cx, |v, cx| {
        v.set_mode(SearchMode::Keyword, cx);
        v.run("salad".into(), cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert_eq!(v.mode(), SearchMode::Keyword);
        assert_eq!(v.local_hits_for_test().len(), 1);
        assert!(v.local_hits_for_test()[0].message.text.contains("salad"));
    });
}

#[test]
fn semantic_indexing_reads_archive_files_without_holding_the_shared_store() {
    use slackless_core::embeddings::{Embedder, HashProjectionEmbedder};
    use slackless_core::store::SemanticIndex;

    use super::query::index_semantic_workspaces;

    /// Records, for every document it embeds, whether the live archive's shared read handle
    /// (the one the UI thread locks for channel history) was free.
    struct LockProbe {
        inner: HashProjectionEmbedder,
        shared: SharedStore,
        free: Mutex<Vec<bool>>,
    }

    impl Embedder for LockProbe {
        fn dims(&self) -> usize {
            self.inner.dims()
        }

        fn model_id(&self) -> &str {
            self.inner.model_id()
        }

        fn embed_document(&self, text: &str) -> Result<Vec<f32>, String> {
            let free = self.shared.try_lock().is_ok();
            self.free.lock().unwrap().push(free);
            self.inner.embed_document(text)
        }
    }

    let message = |ts: &str, text: &str| Message {
        ts: Ts::from(ts),
        text: text.into(),
        ..Default::default()
    };
    let dir = std::env::temp_dir().join(format!(
        "slackless-semantic-index-lock-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // A live workspace: the core's writer, plus the shared read-only handle the UI reads from.
    let live = TeamId::from("T_LIVE");
    let path = dir.join("T_LIVE.sqlite");
    let mut writer = Store::open(&path).unwrap();
    writer
        .upsert_messages(
            &live,
            &ChannelId::from("C1"),
            &[message("1700000001.000000", "Kubernetes rollout plan")],
        )
        .unwrap();
    let shared: SharedStore = Arc::new(Mutex::new(Store::open_read_only(&path).unwrap()));
    // A sample workspace has no archive file, so only its shared handle can be indexed.
    let sample = TeamId::from("T_SAMPLE");
    let mut sample_store = Store::open_in_memory().unwrap();
    sample_store
        .upsert_messages(
            &sample,
            &ChannelId::from("C2"),
            &[message("1700000002.000000", "Lunch menu")],
        )
        .unwrap();
    let sample_store: SharedStore = Arc::new(Mutex::new(sample_store));

    let probe = LockProbe {
        inner: HashProjectionEmbedder::new(64),
        shared: shared.clone(),
        free: Mutex::new(Vec::new()),
    };
    let mut index = SemanticIndex::open_in_memory().unwrap();
    let indexed = index_semantic_workspaces(
        &[(live.clone(), shared), (sample.clone(), sample_store)],
        &[(live.clone(), path)],
        &mut index,
        &probe,
    );
    let LockProbe { free, shared, .. } = probe;
    drop((shared, writer));
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(indexed, Ok((2, 0)));
    assert_eq!(index.workspace_ids().unwrap(), vec![live, sample]);
    assert_eq!(
        free.into_inner().unwrap(),
        [true, true],
        "indexing must leave the shared store free while it embeds"
    );
}

#[gpui::test]
fn search_view_tracks_local_loading_and_rejects_duplicate_remote_pages(cx: &mut TestAppContext) {
    use super::state::{SearchScope, SearchWorkspaceSource};

    let ws_a = TeamId::from("T_RACE_A");
    let mut store_a = Store::open_in_memory().expect("store_a");
    store_a
        .upsert_conversation(
            &ws_a,
            &Conversation {
                id: ChannelId::from("C_R1"),
                name: Some("general".into()),
                is_channel: true,
                ..Default::default()
            },
        )
        .unwrap();
    store_a
        .upsert_message(
            &ws_a,
            &ChannelId::from("C_R1"),
            &Message {
                ts: Ts::from("1700001000.000000"),
                text: "canary deployment complete".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let shared_a: SharedStore = Arc::new(Mutex::new(store_a));

    let mut fx = crate::fixtures::load();
    fx.sample = true;
    let team = Team {
        id: ws_a.clone(),
        name: "RaceWS".into(),
        ..Default::default()
    };
    fx.teams = vec![team.clone()];
    fx.set_rail_workspaces(vec![team]);

    // Archive is None, but AllWorkspaces has workspace_sources -> must still query!
    let view = cx.update(|cx| SearchView::new(Arc::new(fx), None, cx));
    view.update(cx, |v, cx| {
        v.set_workspace_sources_quiet(vec![SearchWorkspaceSource {
            team_id: ws_a.clone(),
            name: "RaceWS".into(),
            store: shared_a.clone(),
        }]);
        v.set_scope(SearchScope::AllWorkspaces, cx);
        v.run("canary".into(), cx);
        assert!(
            v.is_local_loading_for_test(),
            "local_loading must be true while async FTS5 search is in flight"
        );
    });
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert!(!v.is_local_loading_for_test());
        assert_eq!(v.local_hits_for_test().len(), 1);
    });

    // Duplicate or out-of-order remote pages (> 1) must not append duplicate hits.
    view.update(cx, |v, cx| {
        let hit = |ts: &str| RemoteMessageHit {
            channel: ChannelId::from("C_R1"),
            channel_name: Some("general".into()),
            message: Message {
                ts: Ts::from(ts),
                text: format!("hit {ts}"),
                ..Default::default()
            },
        };
        v.set_remote_messages(
            "canary",
            ApiSearchSort::Timestamp,
            1,
            Ok(RemoteMessagePage {
                total: 2,
                page: 1,
                page_count: 2,
                hits: vec![hit("1.0")],
            }),
            cx,
        );
        assert_eq!(v.remote_messages_len_for_test(), 1);
        // Out-of-order page 3 before page 2 is ignored.
        v.set_remote_messages(
            "canary",
            ApiSearchSort::Timestamp,
            3,
            Ok(RemoteMessagePage {
                total: 3,
                page: 3,
                page_count: 3,
                hits: vec![hit("3.0")],
            }),
            cx,
        );
        assert_eq!(v.remote_messages_len_for_test(), 1);
        // Page 2 appends normally.
        v.set_remote_messages(
            "canary",
            ApiSearchSort::Timestamp,
            2,
            Ok(RemoteMessagePage {
                total: 2,
                page: 2,
                page_count: 2,
                hits: vec![hit("2.0")],
            }),
            cx,
        );
        assert_eq!(v.remote_messages_len_for_test(), 2);
        // Duplicate page 2 is rejected.
        v.set_remote_messages(
            "canary",
            ApiSearchSort::Timestamp,
            2,
            Ok(RemoteMessagePage {
                total: 2,
                page: 2,
                page_count: 2,
                hits: vec![hit("2.0")],
            }),
            cx,
        );
        assert_eq!(v.remote_messages_len_for_test(), 2);
    });
}

#[test]
fn transcript_match_excerpt_handles_oversized_query_without_panicking() {
    let long_tx = "a".repeat(150);
    let long_q = "a".repeat(121);
    let excerpt = super::query::transcript_match_excerpt(&long_tx, &long_q);
    assert!(excerpt.is_some());
}
