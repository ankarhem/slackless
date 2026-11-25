use super::*;
use crate::StoreError;
use crate::query::{SearchFilters, SearchSort};
use crate::types::message::BotIcons;
use crate::types::{
    Attachment, Edited, File, FileId, Profile, Reaction, SectionKind, TopicPurpose, UserId,
};

fn ws() -> TeamId {
    TeamId::from("T1")
}

#[test]
fn lists_conversations_and_users_per_workspace() {
    let mut store = Store::open_in_memory().unwrap();
    let conv = |id: &str, name: &str| Conversation {
        id: ChannelId::from(id),
        name: Some(name.into()),
        ..Default::default()
    };
    store
        .upsert_conversation(&ws(), &conv("C2", "random"))
        .unwrap();
    store
        .upsert_conversation(&ws(), &conv("C1", "general"))
        .unwrap();
    store
        .upsert_conversation(&TeamId::from("T2"), &conv("C9", "other"))
        .unwrap();
    let ana = User {
        id: UserId::from("U1"),
        ..Default::default()
    };
    store.upsert_user(&ws(), &ana).unwrap();

    let ids: Vec<_> = store
        .conversations(&ws())
        .unwrap()
        .into_iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(ids, [ChannelId::from("C1"), ChannelId::from("C2")]);
    let users: Vec<_> = store
        .users(&ws())
        .unwrap()
        .into_iter()
        .map(|u| u.id)
        .collect();
    assert_eq!(users, [UserId::from("U1")]);
    assert!(store.users(&TeamId::from("T2")).unwrap().is_empty());
}

fn general() -> ChannelId {
    ChannelId::from("C1")
}

fn msg(ts: &str, user: &str, text: &str) -> Message {
    Message {
        ts: Ts::from(ts),
        user: Some(UserId::from(user)),
        text: text.into(),
        ..Default::default()
    }
}

#[test]
fn local_state_round_trips_per_workspace() {
    let mut store = Store::open_in_memory().unwrap();
    let (a, b) = (TeamId::from("T1"), TeamId::from("T2"));
    assert_eq!(store.local(&a, "last_conversation").unwrap(), None);
    store.set_local(&a, "last_conversation", "C1").unwrap();
    store.set_local(&a, "last_conversation", "C2").unwrap();
    assert_eq!(
        store.local(&a, "last_conversation").unwrap().as_deref(),
        Some("C2")
    );
    assert_eq!(store.local(&b, "last_conversation").unwrap(), None);
}

#[test]
fn migrate_is_idempotent() {
    let mut store = Store::open_in_memory().unwrap();
    assert_eq!(store.schema_version().unwrap(), schema::SCHEMA_VERSION);
    store.migrate().unwrap();
    store.migrate().unwrap();
    assert_eq!(store.schema_version().unwrap(), schema::SCHEMA_VERSION);
    let tables: i64 = store
        .conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name IN
             ('message_revision','message_current','message_fts','conversation','user',
              'team','channel_section','custom_emoji','prefs','local_state','offline_queue',
              'bot_profile','huddle_transcript','huddle_transcript_fts')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tables, 14);
    let legacy_reaction: i64 = store
        .conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'reaction'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(legacy_reaction, 0);
}

#[test]
fn sections_follow_the_next_id_chain_and_append_leftovers() {
    let mut store = Store::open_in_memory().unwrap();
    let section = |id: &str, next: Option<&str>| ChannelSection {
        id: id.to_owned(),
        name: format!("section {id}"),
        kind: SectionKind::Standard,
        emoji: "rocket".to_owned(),
        next_id: next.map(str::to_owned),
        channel_ids: vec![ChannelId::from("C1"), ChannelId::from("D1")],
    };
    // Stored order c, b, a, d: a points at b and d at c, so a is the only section nothing
    // points at. Display: the chain a -> b, then leftovers c, d in stored order.
    store
        .replace_sections(
            &ws(),
            &[
                section("c", None),
                section("b", None),
                section("a", Some("b")),
                section("d", Some("c")),
            ],
        )
        .unwrap();

    let sections = store.sections(&ws()).unwrap();
    let ids: Vec<_> = sections.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["a", "b", "c", "d"]);
    assert_eq!(sections[0].name, "section a");
    assert_eq!(sections[0].emoji, "rocket");
    assert_eq!(
        sections[0].channel_ids,
        [ChannelId::from("C1"), ChannelId::from("D1")]
    );

    // A second replace swaps the whole set: stale sections disappear by omission.
    store
        .replace_sections(&ws(), &[section("e", None)])
        .unwrap();
    let ids: Vec<_> = store
        .sections(&ws())
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(ids, ["e".to_owned()]);
    assert!(store.sections(&TeamId::from("T2")).unwrap().is_empty());
}

#[test]
fn custom_emoji_resolves_aliases_and_drops_unresolvable() {
    let mut store = Store::open_in_memory().unwrap();
    let url = "https://emoji.example/party.png";
    store
        .replace_custom_emoji(
            &ws(),
            &[
                ("yay".to_owned(), "alias:party".to_owned()),
                ("party".to_owned(), url.to_owned()),
                ("meh".to_owned(), "alias:gone".to_owned()),
                ("loop".to_owned(), "alias:loop".to_owned()),
                ("chain".to_owned(), "alias:yay".to_owned()),
            ],
        )
        .unwrap();

    // Name order; `chain` resolves through `yay` to `party`; dangling and cyclic aliases
    // are dropped.
    assert_eq!(
        store.custom_emoji(&ws()).unwrap(),
        vec![
            ("chain".to_owned(), url.to_owned()),
            ("party".to_owned(), url.to_owned()),
            ("yay".to_owned(), url.to_owned()),
        ]
    );
    assert!(store.custom_emoji(&TeamId::from("T2")).unwrap().is_empty());
}

#[test]
fn team_and_prefs_round_trip() {
    let mut store = Store::open_in_memory().unwrap();
    assert!(store.team(&ws()).unwrap().is_none());
    assert!(store.prefs(&ws()).unwrap().is_none());

    let team = |name: &str| Team {
        id: TeamId::from("T1"),
        name: name.to_owned(),
        domain: "acme".to_owned(),
        ..Default::default()
    };
    store.upsert_team(&ws(), &team("Acme")).unwrap();
    store.upsert_team(&ws(), &team("Acme Inc")).unwrap();
    assert_eq!(store.team(&ws()).unwrap().unwrap().name, "Acme Inc");

    let prefs = serde_json::json!({"display_real_names": true});
    store.replace_prefs(&ws(), &prefs).unwrap();
    assert_eq!(store.prefs(&ws()).unwrap(), Some(prefs));
}

#[test]
fn connected_teams_list_behind_the_own_team() {
    let mut store = Store::open_in_memory().unwrap();
    assert!(store.teams(&ws()).unwrap().is_empty());

    let team = |id: &str, name: &str| Team {
        id: TeamId::from(id),
        name: name.to_owned(),
        domain: name.to_lowercase(),
        ..Default::default()
    };
    // The external org sorts before the own id, so a plain scan would shadow it.
    store.upsert_team(&ws(), &team("T0AEXT", "Zeta")).unwrap();
    store
        .upsert_team(&ws(), &team(ws().as_str(), "Acme"))
        .unwrap();
    store.upsert_team(&ws(), &team("T0BEXT", "Beta")).unwrap();

    assert_eq!(
        store.team(&ws()).unwrap().unwrap().name,
        "Acme",
        "the own record stays answerable with connected teams stored beside it"
    );
    let names: Vec<_> = store
        .teams(&ws())
        .unwrap()
        .into_iter()
        .map(|team| team.name)
        .collect();
    assert_eq!(names, ["Acme", "Beta", "Zeta"], "own first, then by name");
}

#[test]
fn member_team_ids_lists_only_external_distinct_teams() {
    let mut store = Store::open_in_memory().unwrap();
    let user = |id: &str, team: Option<&str>| User {
        id: UserId::from(id),
        team_id: team.map(TeamId::from),
        ..Default::default()
    };
    store.upsert_user(&ws(), &user("U1", Some("T1"))).unwrap();
    store
        .upsert_user(&ws(), &user("U2", Some("T0EXT")))
        .unwrap();
    store
        .upsert_user(&ws(), &user("U3", Some("T0EXT")))
        .unwrap();
    store.upsert_user(&ws(), &user("U4", None)).unwrap();

    let members = ["U1", "U2", "U3", "U4", "U9", "USLACKBOT"]
        .map(UserId::from)
        .to_vec();
    assert_eq!(
        store.member_team_ids(&ws(), &members).unwrap(),
        vec![TeamId::from("T0EXT")],
        "own team, duplicates, missing profiles and USLACKBOT never list"
    );
}

#[test]
fn upsert_round_trips_through_channel_history() {
    let mut store = Store::open_in_memory().unwrap();
    let first = msg("1700000000.000100", "U1", "first");
    let second = msg("1700000001.000200", "U2", "second");
    let mut reply = msg("1700000002.000300", "U1", "in thread");
    reply.thread_ts = Some(first.ts.clone());
    for m in [&second, &first, &reply] {
        store.upsert_message(&ws(), &general(), m).unwrap();
    }
    // Re-observing a message is a no-op for the log.
    store.upsert_message(&ws(), &general(), &first).unwrap();

    let history = store.channel_history(&ws(), &general(), None, 50).unwrap();
    let texts: Vec<_> = history.iter().map(|m| m.message.text.as_str()).collect();
    assert_eq!(texts, ["first", "second"]);
    assert_eq!(history[0].message.channel, Some(general()));
    assert!(!history[0].deleted);
    assert_eq!(history[0].message.ts, first.ts);

    let older = store
        .channel_history(&ws(), &general(), Some(&second.ts), 50)
        .unwrap();
    assert_eq!(older.len(), 1);
    let thread = store.thread_replies(&ws(), &general(), &first.ts).unwrap();
    assert_eq!(thread.len(), 2);
    assert_eq!(
        store.revisions(&ws(), &general(), &first.ts).unwrap().len(),
        1
    );
}

#[test]
fn latest_messages_picks_each_channels_newest_top_level() {
    let mut store = Store::open_in_memory().unwrap();
    let (dm1, dm2) = (ChannelId::from("D1"), ChannelId::from("D2"));
    let older = msg("1700000000.000100", "U1", "older");
    let newest = msg("1700000002.000200", "U2", "newest");
    let mut reply = msg("1700000003.000300", "U1", "in thread");
    reply.thread_ts = Some(newest.ts.clone());
    for m in [&older, &newest, &reply] {
        store.upsert_message(&ws(), &dm1, m).unwrap();
    }
    store
        .upsert_message(&ws(), &dm2, &msg("1700000001.000100", "U2", "other dm"))
        .unwrap();
    // Another workspace's message must not leak into the batch.
    store
        .upsert_message(
            &TeamId::from("T2"),
            &dm1,
            &msg("1800000000.000000", "U1", "elsewhere"),
        )
        .unwrap();

    let latest = store
        .latest_messages(&ws(), &[dm1.clone(), dm2.clone(), ChannelId::from("D3")])
        .unwrap();
    assert_eq!(
        latest.len(),
        2,
        "only the asked channels; empty D3 stays absent"
    );
    assert_eq!(
        latest[&dm1].message.text, "newest",
        "a reply is not top-level"
    );
    assert!(!latest[&dm1].deleted);
    assert_eq!(latest[&dm1].message.channel, Some(dm1.clone()));
    assert_eq!(latest[&dm2].message.text, "other dm");

    // A deleted message is never the latest: dm2's only message gone, it has none.
    store
        .record_deletion(&ws(), &dm2, &Ts::from("1700000001.000100"), None, None)
        .unwrap();
    let latest = store
        .latest_messages(&ws(), &[dm1.clone(), dm2.clone()])
        .unwrap();
    assert!(!latest.contains_key(&dm2));
    assert!(latest.contains_key(&dm1));

    assert!(store.latest_messages(&ws(), &[]).unwrap().is_empty());
}

#[test]
fn member_counts_split_humans_apps_and_drop_the_gone() {
    let mut store = Store::open_in_memory().unwrap();
    let user = |id: &str, bot: bool, deleted: bool| User {
        id: UserId::from(id),
        is_bot: bot,
        deleted,
        ..Default::default()
    };
    store.upsert_user(&ws(), &user("U1", false, false)).unwrap();
    store
        .upsert_user(&ws(), &user("UBOT", true, false))
        .unwrap();
    store
        .upsert_user(&ws(), &user("UDEADBOT", true, true))
        .unwrap();
    store
        .upsert_user(&ws(), &user("UGONE", false, true))
        .unwrap();

    let members = [
        "U1",
        "UBOT",
        "UDEADBOT",
        "UGONE",
        // No stored profile: an unknown person is still a person.
        "UEXT",
        // The workspace bot is neither a human nor an app.
        "USLACKBOT",
    ]
    .map(UserId::from)
    .to_vec();

    assert_eq!(
        store.member_counts(&ws(), &members).unwrap(),
        MemberCounts { humans: 2, apps: 1 }
    );
    // Another workspace's profiles never leak in: everyone is an unknown human there.
    assert_eq!(
        store.member_counts(&TeamId::from("T2"), &members).unwrap(),
        MemberCounts { humans: 5, apps: 0 }
    );
    assert_eq!(
        store
            .member_counts(&ws(), &[UserId::from("USLACKBOT")])
            .unwrap(),
        MemberCounts::default()
    );
}

#[test]
fn channels_with_messages_counts_deleted_ones_as_stored() {
    let mut store = Store::open_in_memory().unwrap();
    let dm1 = ChannelId::from("D1");
    store
        .upsert_message(&ws(), &dm1, &msg("1700000000.000100", "U1", "only one"))
        .unwrap();
    let dm2 = ChannelId::from("D2");
    store
        .upsert_message(&ws(), &dm2, &msg("1700000001.000100", "U1", "soon gone"))
        .unwrap();
    store
        .record_deletion(&ws(), &dm2, &Ts::from("1700000001.000100"), None, None)
        .unwrap();

    let asked = [dm1.clone(), dm2.clone(), ChannelId::from("D3")];
    assert_eq!(
        store.channels_with_messages(&ws(), &asked).unwrap(),
        [dm1, dm2].into_iter().collect(),
        "D2's deleted message still means its history was fetched once"
    );
    assert!(store.channels_with_messages(&ws(), &[]).unwrap().is_empty());
}

#[test]
fn set_channel_memberships_flips_channel_rows_only_and_only_once() {
    let mut store = Store::open_in_memory().unwrap();
    let channel = |id: &str, member: bool, private: bool| Conversation {
        id: ChannelId::from(id),
        is_channel: !private,
        is_private: private,
        is_member: member,
        ..Default::default()
    };
    store
        .upsert_conversation(&ws(), &channel("C1", false, false))
        .unwrap();
    store
        .upsert_conversation(&ws(), &channel("C2", true, false))
        .unwrap();
    store
        .upsert_conversation(&ws(), &channel("C3", true, true))
        .unwrap();
    store
        .upsert_conversation(
            &ws(),
            &Conversation {
                id: ChannelId::from("M1"),
                is_mpim: true,
                is_member: true,
                ..Default::default()
            },
        )
        .unwrap();

    let mut flipped = store
        .set_channel_memberships(&ws(), &[ChannelId::from("C1")])
        .unwrap();
    flipped.sort();
    assert_eq!(
        flipped,
        vec![
            ChannelId::from("C1"),
            ChannelId::from("C2"),
            ChannelId::from("C3"),
        ],
        "joiner up, both leavers down"
    );
    let read = |store: &Store, id: &str| {
        store
            .conversation(&ws(), &ChannelId::from(id))
            .unwrap()
            .unwrap_or_else(|| panic!("{id} stored"))
    };
    assert!(
        read(&store, "C1").is_member,
        "the json copy follows the column"
    );
    assert!(!read(&store, "C2").is_member);
    assert!(!read(&store, "C3").is_member);
    assert!(read(&store, "M1").is_member, "group DM rows are untouched");

    // An unchanged pass rewrites nothing: no flips, not even updated_at.
    let stamps = |store: &Store| -> Vec<(String, i64)> {
        let mut stmt = store
            .conn
            .prepare("SELECT id, updated_at FROM conversation ORDER BY id")
            .unwrap();
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .unwrap();
        rows.collect::<rusqlite::Result<_>>().unwrap()
    };
    let before = stamps(&store);
    assert!(
        store
            .set_channel_memberships(&ws(), &[ChannelId::from("C1")])
            .unwrap()
            .is_empty()
    );
    assert_eq!(stamps(&store), before, "no churn when nothing flips");

    // An empty membership list resets every channel row.
    let flipped = store.set_channel_memberships(&ws(), &[]).unwrap();
    assert_eq!(flipped, vec![ChannelId::from("C1")]);
    assert!(!read(&store, "C1").is_member);
    assert!(read(&store, "M1").is_member);
}

#[test]
fn edit_appends_revision_and_updates_current() {
    let mut store = Store::open_in_memory().unwrap();
    let original = msg("1700000000.000100", "U1", "teh original");
    store.upsert_message(&ws(), &general(), &original).unwrap();
    let mut edited = msg("1700000000.000100", "U1", "the edited");
    edited.edited = Some(Edited {
        user: Some(UserId::from("U1")),
        ts: Ts::from("1700000050.000000"),
    });
    store.upsert_message(&ws(), &general(), &edited).unwrap();
    // A stale observation of the original must not roll the current state back.
    store.upsert_message(&ws(), &general(), &original).unwrap();

    let history = store.channel_history(&ws(), &general(), None, 10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].message.text, "the edited");

    let revisions = store.revisions(&ws(), &general(), &original.ts).unwrap();
    let texts: Vec<_> = revisions.iter().map(|r| r.message.text.as_str()).collect();
    assert_eq!(texts, ["teh original", "the edited"]);
    assert_eq!(revisions[1].edit_ts, "1700000050.000000");

    // Search follows the current text only.
    assert!(
        store
            .search(&ws(), &SearchQuery::parse("teh"), 10)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .search(&ws(), &SearchQuery::parse("edited"), 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn deletion_keeps_original_content_in_log() {
    let mut store = Store::open_in_memory().unwrap();
    let original = msg("1700000000.000100", "U1", "secret plans");
    store.upsert_message(&ws(), &general(), &original).unwrap();
    store
        .record_deletion(
            &ws(),
            &general(),
            &original.ts,
            Some(&Ts::from("1700000099.000000")),
            None,
        )
        .unwrap();
    // Late history replay does not resurrect it.
    store.upsert_message(&ws(), &general(), &original).unwrap();

    let history = store.channel_history(&ws(), &general(), None, 10).unwrap();
    assert_eq!(history.len(), 1);
    assert!(history[0].deleted);
    assert_eq!(history[0].message.text, "secret plans");

    let revisions = store.revisions(&ws(), &general(), &original.ts).unwrap();
    assert_eq!(revisions.len(), 2);
    assert!(!revisions[0].deleted);
    assert_eq!(revisions[0].message.text, "secret plans");
    assert!(revisions[1].deleted);
    assert_eq!(revisions[1].message.text, "secret plans");

    // The log refuses mutation.
    assert!(
        store
            .conn
            .execute("DELETE FROM message_revision", [])
            .is_err()
    );
    assert!(
        store
            .search(&ws(), &SearchQuery::parse("secret"), 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn fts_search_with_modifiers() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .upsert_conversation(
            &ws(),
            &Conversation {
                id: general(),
                name: Some("general".into()),
                is_channel: true,
                ..Default::default()
            },
        )
        .unwrap();
    store
        .upsert_user(
            &ws(),
            &User {
                id: UserId::from("U2"),
                name: "ana".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let mut deploy = msg(
        "1704153600.000100",
        "U2",
        "Deploying the rocket today https://x.io",
    );
    deploy.reactions = vec![Reaction {
        name: "rocket".into(),
        count: 1,
        users: vec![UserId::from("U1")],
    }];
    let mut report = msg("1704240000.000100", "U1", "rocket report attached");
    report.files = vec![File {
        id: FileId::from("F1"),
        name: Some("launch-metrics.pdf".into()),
        ..Default::default()
    }];
    store.upsert_message(&ws(), &general(), &deploy).unwrap();
    store.upsert_message(&ws(), &general(), &report).unwrap();
    store
        .upsert_message(
            &ws(),
            &ChannelId::from("C2"),
            &msg("1704240001.0", "U2", "rocket elsewhere"),
        )
        .unwrap();

    let texts = |q: &str| -> Vec<String> {
        store
            .search(&ws(), &SearchQuery::parse(q), 10)
            .unwrap()
            .into_iter()
            .map(|m| m.message.text)
            .collect()
    };
    assert_eq!(texts("deploying").len(), 1);
    assert_eq!(texts("rocket").len(), 3);
    assert_eq!(texts("rocket in:general").len(), 2);
    assert_eq!(
        texts("rocket in:#general from:@ana"),
        ["Deploying the rocket today https://x.io"]
    );
    assert_eq!(texts("rocket in:C2 from:U2"), ["rocket elsewhere"]);
    assert_eq!(texts("launch"), ["rocket report attached"]);
    assert_eq!(texts("has:file").len(), 1);
    assert_eq!(
        texts("has:link"),
        ["Deploying the rocket today https://x.io"]
    );
    assert_eq!(texts("has:reaction").len(), 1);
    assert_eq!(
        texts("rocket after:2024-01-02 in:general"),
        ["rocket report attached"]
    );
    assert_eq!(
        texts("rocket before:2024-01-03"),
        ["Deploying the rocket today https://x.io"]
    );
    assert!(texts("nonexistentword").is_empty());
    assert!(
        texts(r#"rocket" OR "x"#).is_empty(),
        "FTS syntax in input is literal"
    );
}

#[test]
fn filter_chip_constraints() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .upsert_conversation(
            &ws(),
            &Conversation {
                id: general(),
                name: Some("general".into()),
                is_channel: true,
                ..Default::default()
            },
        )
        .unwrap();
    let mut mention = msg("1704153600.000100", "U2", "ping <@U1> about the rocket");
    mention.thread_ts = Some(Ts::from("1704153600.000000"));
    let plain = msg("1704240000.000100", "U1", "plain rocket");
    let top = msg("1704240010.000100", "U2", "top-level rocket <@U2>");
    store.upsert_message(&ws(), &general(), &mention).unwrap();
    store.upsert_message(&ws(), &general(), &plain).unwrap();
    store.upsert_message(&ws(), &general(), &top).unwrap();

    let found = |filters: SearchFilters| -> Vec<String> {
        let mut query = SearchQuery::parse("rocket");
        query.filters = filters;
        store
            .search(&ws(), &query, 10)
            .unwrap()
            .into_iter()
            .map(|m| m.message.text)
            .collect()
    };
    let threads = SearchFilters {
        threads_only: true,
        ..Default::default()
    };
    assert_eq!(found(threads), ["ping <@U1> about the rocket"]);
    let mentions_me = SearchFilters {
        includes_user: Some("U1".into()),
        ..Default::default()
    };
    assert_eq!(found(mentions_me), ["ping <@U1> about the rocket"]);
    let both = SearchFilters {
        includes_user: Some("U1".into()),
        threads_only: true,
        ..Default::default()
    };
    assert_eq!(found(both).len(), 1);
    let nobody = SearchFilters {
        includes_user: Some("U9".into()),
        ..Default::default()
    };
    assert!(found(nobody).is_empty());
}

#[test]
fn recent_messages_by_user_returns_newest_undeleted_for_user() {
    let mut store = Store::open_in_memory().unwrap();
    let m1 = msg("1700000001.000100", "U1", "first");
    let m2 = msg("1700000002.000100", "U2", "other user");
    let m3 = msg("1700000003.000100", "U1", "second");
    let m4 = msg("1700000004.000100", "U1", "deleted one");
    store.upsert_message(&ws(), &general(), &m1).unwrap();
    store.upsert_message(&ws(), &general(), &m2).unwrap();
    store
        .upsert_message(&ws(), &ChannelId::from("C2"), &m3)
        .unwrap();
    store.upsert_message(&ws(), &general(), &m4).unwrap();
    store
        .record_deletion(&ws(), &general(), &m4.ts, None, Some(&m4))
        .unwrap();

    let rows = store
        .recent_messages_by_user(&ws(), &UserId::from("U1"), 10)
        .unwrap();
    let texts: Vec<&str> = rows.iter().map(|r| r.message.text.as_str()).collect();
    assert_eq!(texts, ["second", "first"]);
}

#[test]
fn ts_key_is_numeric() {
    assert_eq!(
        ts_key(&Ts::from("1700000000.000100")),
        1_700_000_000_000_100
    );
    assert_eq!(ts_key(&Ts::from("1700000000.1")), 1_700_000_000_100_000);
    assert_eq!(ts_key(&Ts::from("1700000000")), 1_700_000_000_000_000);
}

#[test]
fn stats_vacuum_and_offline_queue_round_trip() {
    let mut store = Store::open_in_memory().unwrap();
    let m1 = msg("1700000001.000100", "U1", "hello storage");
    store.upsert_message(&ws(), &general(), &m1).unwrap();
    let id1 = store.enqueue_offline(&ws(), "{\"a\":1}").unwrap();
    let id2 = store.enqueue_offline(&ws(), "{\"b\":2}").unwrap();
    assert!(id2 > id1);

    let stats = store.stats(&ws()).unwrap();
    assert_eq!(stats.messages, 1);
    assert_eq!(stats.revisions, 1);
    assert_eq!(stats.offline_queued, 2);
    assert!(stats.db_bytes() > 0);
    let breakdown = store.storage_breakdown(&ws(), 5).unwrap();
    assert_eq!(breakdown.top_channels.len(), 1);
    assert_eq!(breakdown.top_channels[0].0, "C1");
    assert_eq!(breakdown.top_channels[0].2, 1);

    assert_eq!(
        store.list_offline(&ws()).unwrap(),
        vec![(id1, "{\"a\":1}".to_owned()), (id2, "{\"b\":2}".to_owned())]
    );
    store.delete_offline(id1).unwrap();
    assert_eq!(
        store.list_offline(&ws()).unwrap(),
        vec![(id2, "{\"b\":2}".to_owned())]
    );
    store.clear_offline(&ws()).unwrap();
    assert!(store.list_offline(&ws()).unwrap().is_empty());

    let mut thread_root = msg("1700000002.000100", "U1", "thread starter");
    thread_root.reply_count = Some(3);
    store
        .upsert_message(&ws(), &general(), &thread_root)
        .unwrap();
    let threads = store.recent_threads(&ws(), None, 10).unwrap();
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].message.text, "thread starter");
    assert!(
        store
            .recent_threads(&ws(), Some(&ChannelId::from("C9")), 10)
            .unwrap()
            .is_empty()
    );

    store.vacuum().unwrap();
}

#[test]
fn all_local_and_read_only_wal_reader_see_committed_writes() {
    let dir = std::env::temp_dir().join(format!("slackless-store-ro-test-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("test.sqlite");
    let _ = std::fs::remove_file(&path);

    let mut writer = Store::open(&path).unwrap();
    let reader = Store::open_read_only(&path).unwrap();
    writer.set_local(&ws(), "font", "Lato").unwrap();
    writer.set_local(&ws(), "zoom", "3").unwrap();
    let all = reader.all_local(&ws()).unwrap();
    assert_eq!(all.get("font").map(String::as_str), Some("Lato"));
    assert_eq!(all.get("zoom").map(String::as_str), Some("3"));

    drop(reader);
    drop(writer);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn compressed_json_round_trips_and_vacuum_compacts_legacy_text() {
    let mut store = Store::open_in_memory().unwrap();
    let long_text = "Slackless archive payload compression test ".repeat(24);
    let m1 = msg("1700000100.000100", "U1", &long_text);
    store.upsert_message(&ws(), &general(), &m1).unwrap();

    // Untouched original lives in `message_current` as a compressed BLOB without duplicating
    // into `message_revision`, yet `revisions()` still returns the single original revision.
    let kind: String = store
        .conn
        .query_row(
            "SELECT typeof(json) FROM message_current WHERE ts = '1700000100.000100'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(kind, "blob");
    let rev_count: i64 = store
        .conn
        .query_row("SELECT count(*) FROM message_revision", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rev_count, 0);
    let revs = store.revisions(&ws(), &general(), &m1.ts).unwrap();
    assert_eq!(revs.len(), 1);
    assert_eq!(revs[0].message.text, long_text);

    // Simulate a legacy uncompressed TEXT row and verify `vacuum()` compresses it in place.
    let legacy_json = serde_json::to_string(&m1).unwrap();
    store
        .conn
        .execute(
            "UPDATE message_current SET json = ?1 WHERE ts = '1700000100.000100'",
            params![legacy_json],
        )
        .unwrap();
    store.vacuum().unwrap();
    let kind_after: String = store
        .conn
        .query_row(
            "SELECT typeof(json) FROM message_current WHERE ts = '1700000100.000100'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(kind_after, "blob");
    let hist = store.channel_history(&ws(), &general(), None, 10).unwrap();
    assert_eq!(hist.len(), 1);
    assert_eq!(hist[0].message.text, long_text);
}

#[test]
fn chunked_in_queries_and_batch_upserts_handle_large_sets() {
    let mut store = Store::open_in_memory().unwrap();
    let n = 1_100usize;
    let users: Vec<User> = (0..n)
        .map(|i| User {
            id: UserId::from(format!("U{i:05}")),
            name: format!("user{i}"),
            is_bot: i.is_multiple_of(10),
            deleted: i.is_multiple_of(25),
            team_id: Some(if i.is_multiple_of(50) {
                TeamId::from("TEXTERNAL")
            } else {
                ws()
            }),
            ..Default::default()
        })
        .collect();
    let convs: Vec<Conversation> = (0..n)
        .map(|i| Conversation {
            id: ChannelId::from(format!("C{i:05}")),
            name: Some(format!("chan-{i}")),
            is_channel: true,
            is_member: i % 2 == 0,
            ..Default::default()
        })
        .collect();

    store.upsert_users(&ws(), &users).unwrap();
    store.upsert_conversations(&ws(), &convs).unwrap();

    let user_ids: Vec<UserId> = users.iter().map(|u| u.id.clone()).collect();
    let counts = store.member_counts(&ws(), &user_ids).unwrap();
    assert!(counts.humans > 0 && counts.apps > 0);
    let ext_teams = store.member_team_ids(&ws(), &user_ids).unwrap();
    assert_eq!(ext_teams, vec![TeamId::from("TEXTERNAL")]);

    let chan_ids: Vec<ChannelId> = convs.iter().map(|c| c.id.clone()).collect();
    let flags = store.member_flags(&ws(), &chan_ids).unwrap();
    assert_eq!(flags.len(), n / 2);

    // Flip memberships via temp-table path (> SQL_IN_CHUNK) so only odd channels are members.
    let odd_ids: Vec<ChannelId> = chan_ids
        .iter()
        .enumerate()
        .filter(|(i, _)| i % 2 == 1)
        .map(|(_, id)| id.clone())
        .collect();
    let flipped = store.set_channel_memberships(&ws(), &odd_ids).unwrap();
    assert_eq!(flipped.len(), n);
    assert_eq!(store.member_flags(&ws(), &chan_ids).unwrap().len(), n / 2);

    // Batch upsert_messages + single-row PK Store::message lookup + chunked channels_with_messages.
    let batch = vec![
        msg("1700001001.000100", "U00001", "batch one"),
        msg("1700001002.000100", "U00002", "batch two"),
    ];
    store.upsert_messages(&ws(), &chan_ids[0], &batch).unwrap();
    let found = store
        .message(&ws(), &chan_ids[0], &Ts::from("1700001002.000100"))
        .unwrap()
        .expect("single-row PK lookup finds stored message");
    assert_eq!(found.message.text, "batch two");
    let with_msgs = store.channels_with_messages(&ws(), &chan_ids).unwrap();
    assert_eq!(with_msgs.len(), 1);
    assert!(with_msgs.contains(&chan_ids[0]));
}

#[test]
fn ephemeral_messages_stay_uncompressed_through_vacuum_and_drop_ephemeral_removes_small_and_large()
{
    let mut store = Store::open_in_memory().unwrap();

    // Helper checks both first-class `is_ephemeral` field and legacy `extra["is_ephemeral"]`.
    let mut field_eph = msg("1700000001.000100", "U1", "ephemeral field");
    field_eph.is_ephemeral = true;
    assert!(is_ephemeral_message(&field_eph));

    let mut extra_eph = msg("1700000002.000100", "U1", "ephemeral extra");
    extra_eph
        .extra
        .insert("is_ephemeral".into(), serde_json::Value::Bool(true));
    assert!(is_ephemeral_message(&extra_eph));
    assert!(!is_ephemeral_message(&msg(
        "1700000003.000100",
        "U1",
        "normal"
    )));

    // 1. Large (>=160B) ephemeral message with an attachment (the exact case the bug broke).
    let mut large_eph = msg(
        "1700000010.000100",
        "U1",
        &"Only visible to you preview payload ".repeat(12),
    );
    large_eph.is_ephemeral = true;
    large_eph.attachments.push(Attachment {
        title: Some("Giphy preview".into()),
        text: Some("Click Send or Shuffle ".repeat(10)),
        from_url: Some("https://giphy.example/abc".into()),
        ..Default::default()
    });
    store.upsert_message(&ws(), &general(), &large_eph).unwrap();

    // 2. Small (<160B) ephemeral row stored directly as compact TEXT JSON.
    let small_eph_json = r#"{"ts":"1700000011.000100","text":"ephemeral hi","is_ephemeral":true}"#;
    assert!(small_eph_json.len() < 160);
    store
        .conn
        .execute(
            "INSERT INTO message_current
             (workspace_id, channel_id, ts, ts_key, edit_ts, user_id, text, file_names, deleted,
              has_link, has_file, is_pinned, reply_count, json, updated_at)
             VALUES
             ('T1','C1','1700000011.000100',1700000011000100,'','U1','ephemeral hi','',0,0,0,0,NULL,?1,100)",
            params![small_eph_json],
        )
        .unwrap();

    // 3. Large non-ephemeral legacy TEXT row (should be compressed by vacuum()).
    let large_normal = msg(
        "1700000012.000100",
        "U1",
        &"Regular archived channel message ".repeat(16),
    );
    store
        .upsert_message(&ws(), &general(), &large_normal)
        .unwrap();
    let legacy_normal_json = serde_json::to_string(&large_normal).unwrap();
    assert!(legacy_normal_json.len() >= 160);
    store
        .conn
        .execute(
            "UPDATE message_current SET json = ?1 WHERE ts = '1700000012.000100'",
            params![legacy_normal_json],
        )
        .unwrap();

    // 4. >=160B incompressible non-ephemeral row (random high-entropy ASCII so zlib cannot beat
    //    the 16-byte threshold; vacuum() must leave it as TEXT).
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    let alphabet: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz!#$%&()*+,-./:;<=>?@[]^_`{|}~";
    let mut entropy = String::with_capacity(140);
    for _ in 0..140 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let idx = ((state >> 33) as usize) % alphabet.len();
        entropy.push(alphabet[idx] as char);
    }
    let incompressible_json = format!(r#"{{"ts":"1700000013.000100","text":"{entropy}"}}"#);
    assert!(incompressible_json.len() >= 160);
    assert!(
        matches!(
            encode_json_value(&incompressible_json, false),
            SqlValue::Text(_)
        ),
        "test fixture must be incompressible"
    );
    store
        .conn
        .execute(
            "INSERT INTO message_current
             (workspace_id, channel_id, ts, ts_key, edit_ts, user_id, text, file_names, deleted,
              has_link, has_file, is_pinned, reply_count, json, updated_at)
             VALUES
             ('T1','C1','1700000013.000100',1700000013000100,'','U1',?1,'',0,0,0,0,NULL,?2,100)",
            params![entropy, incompressible_json],
        )
        .unwrap();

    // 5. Ephemeral message in another workspace (must not be dropped when dropping T1).
    let mut other_ws_eph = msg("1700000014.000100", "U1", "other workspace ephemeral");
    other_ws_eph.is_ephemeral = true;
    store
        .upsert_message(&TeamId::from("T2"), &general(), &other_ws_eph)
        .unwrap();

    let json_type = |store: &Store, ts: &str| -> String {
        store
            .conn
            .query_row(
                "SELECT typeof(json) FROM message_current WHERE ts = ?1",
                [ts],
                |r| r.get(0),
            )
            .unwrap()
    };

    // Before vacuum: both ephemerals are stored as uncompressed TEXT.
    assert_eq!(json_type(&store, "1700000010.000100"), "text");
    assert_eq!(json_type(&store, "1700000011.000100"), "text");
    assert_eq!(json_type(&store, "1700000012.000100"), "text");
    assert_eq!(json_type(&store, "1700000013.000100"), "text");

    // Vacuum compresses the >=160B non-ephemeral row, but leaves both ephemerals & the
    // incompressible row as TEXT.
    store.vacuum().unwrap();
    assert_eq!(json_type(&store, "1700000010.000100"), "text");
    assert_eq!(json_type(&store, "1700000011.000100"), "text");
    assert_eq!(json_type(&store, "1700000012.000100"), "blob");
    assert_eq!(json_type(&store, "1700000013.000100"), "text");

    // drop_ephemeral removes both small and large ephemerals in T1, bumps revision, and keeps
    // non-ephemerals and T2 ephemerals intact.
    let rev_before = Store::message_revision();
    let dropped = store.drop_ephemeral(&ws()).unwrap();
    assert_eq!(dropped, 2);
    assert!(Store::message_revision() > rev_before);
    assert_eq!(store.drop_ephemeral(&ws()).unwrap(), 0);

    assert!(
        store
            .message(&ws(), &general(), &Ts::from("1700000010.000100"))
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .message(&ws(), &general(), &Ts::from("1700000011.000100"))
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .message(&ws(), &general(), &Ts::from("1700000012.000100"))
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .message(&ws(), &general(), &Ts::from("1700000013.000100"))
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .message(
                &TeamId::from("T2"),
                &general(),
                &Ts::from("1700000014.000100")
            )
            .unwrap()
            .is_some()
    );
}

#[test]
fn search_sorted_relevance_exclusions_wildcards_null_user_and_modifiers() {
    let mut store = Store::open_in_memory().unwrap();

    for (id, name) in [("C1", "general"), ("C2", "alerts"), ("C3", "deploys")] {
        store
            .upsert_conversation(
                &ws(),
                &Conversation {
                    id: ChannelId::from(id),
                    name: Some(name.into()),
                    is_channel: true,
                    ..Default::default()
                },
            )
            .unwrap();
    }

    store
        .upsert_user(
            &ws(),
            &User {
                id: UserId::from("UALICE"),
                name: "alice".into(),
                real_name: Some("Alice Lovelace".into()),
                profile: Profile {
                    display_name: Some("ally".into()),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
    store
        .upsert_user(
            &ws(),
            &User {
                id: UserId::from("UBOB"),
                name: "bob".into(),
                real_name: Some("Bob Builder".into()),
                profile: Profile {
                    display_name: Some("bobby".into()),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();

    // Older message with dense "deploy" occurrences (should win BM25 Relevant sort).
    let mut dense = msg(
        "1704100000.000100",
        "UALICE",
        "deploy deploy deploy production deployment checklist",
    );
    dense.pinned_to = vec![ChannelId::from("C1")];
    store
        .upsert_message(&ws(), &ChannelId::from("C1"), &dense)
        .unwrap();

    // Newer message with a single "deploy" mention + staging exclusion test.
    let staging = msg(
        "1704200000.000100",
        "UBOB",
        "deploy to staging dry run complete",
    );
    store
        .upsert_message(&ws(), &ChannelId::from("C2"), &staging)
        .unwrap();

    // Bot/webhook message with user_id = None and attachment from_url link.
    let bot_from_url = Message {
        ts: Ts::from("1704300000.000100"),
        user: None,
        bot_id: Some(BotId::from("BHOOK")),
        text: "deploying release v2 automatically".into(),
        attachments: vec![Attachment {
            from_url: Some("https://ci.example/build/42".into()),
            ..Default::default()
        }],
        ..Default::default()
    };
    store
        .upsert_message(&ws(), &ChannelId::from("C3"), &bot_from_url)
        .unwrap();

    // Another message with attachment title_link only (no URL in text).
    let mut title_link_msg = msg("1704400000.000100", "UBOB", "quarterly notes ready");
    title_link_msg.attachments = vec![Attachment {
        title_link: Some("https://docs.example/q4".into()),
        ..Default::default()
    }];
    store
        .upsert_message(&ws(), &ChannelId::from("C1"), &title_link_msg)
        .unwrap();

    let run = |q: &str, sort: SearchSort| -> Vec<String> {
        store
            .search_sorted(&ws(), &SearchQuery::parse(q), sort, 20)
            .unwrap()
            .into_iter()
            .map(|m| m.message.ts.as_str().to_owned())
            .collect()
    };

    // SearchSort::Recent orders newest first; SearchSort::Relevant ranks denser BM25 match first.
    assert_eq!(
        run("deploy", SearchSort::Recent),
        vec!["1704200000.000100", "1704100000.000100"]
    );
    assert_eq!(
        run("deploy", SearchSort::Relevant),
        vec!["1704100000.000100", "1704200000.000100"]
    );

    // Prefix wildcard matches both "deploy" and "deploying".
    assert_eq!(
        run("deploy*", SearchSort::Recent),
        vec![
            "1704300000.000100",
            "1704200000.000100",
            "1704100000.000100"
        ]
    );

    // Positive + excluded FTS term and phrase.
    assert_eq!(
        run("deploy* -staging", SearchSort::Recent),
        vec!["1704300000.000100", "1704100000.000100"]
    );
    assert_eq!(
        run(r#"deploy -"dry run""#, SearchSort::Recent),
        vec!["1704100000.000100"]
    );

    // Negative-only FTS query excludes rows matching "staging" and keeps all others.
    assert_eq!(
        run("-staging", SearchSort::Recent),
        vec![
            "1704400000.000100",
            "1704300000.000100",
            "1704100000.000100"
        ]
    );

    // -from:@alice excludes Alice AND preserves the bot/webhook message (user_id IS NULL)!
    assert_eq!(
        run("-from:@alice", SearchSort::Recent),
        vec![
            "1704400000.000100",
            "1704300000.000100",
            "1704200000.000100"
        ]
    );
    // Multiple -from exclusions still preserve NULL user_id messages.
    assert_eq!(
        run("-from:@alice -from:@bob", SearchSort::Recent),
        vec!["1704300000.000100"]
    );

    // -in:#alerts excludes C2 while preserving C1 and C3.
    assert_eq!(
        run("-in:#alerts", SearchSort::Recent),
        vec![
            "1704400000.000100",
            "1704300000.000100",
            "1704100000.000100"
        ]
    );

    // Has::Pin matches pinned_to messages.
    assert_eq!(
        run("has:pin", SearchSort::Recent),
        vec!["1704100000.000100"]
    );

    // Has::Link matches attachments[].from_url AND attachments[].title_link.
    assert_eq!(
        run("has:link", SearchSort::Recent),
        vec!["1704400000.000100", "1704300000.000100"]
    );

    // Multi-value OR: in:C2 in:C3 matches either channel.
    assert_eq!(
        run("in:#alerts in:#deploys", SearchSort::Recent),
        vec!["1704300000.000100", "1704200000.000100"]
    );

    // from: matches display_name and real_name (case-insensitive), including quoted values.
    assert_eq!(
        run("from:@ALLY", SearchSort::Recent),
        vec!["1704100000.000100"]
    );
    assert_eq!(
        run(r#"from:"Bob Builder""#, SearchSort::Recent),
        vec!["1704400000.000100", "1704200000.000100"]
    );
    assert_eq!(
        run(r#"from:"alice lovelace""#, SearchSort::Recent),
        vec!["1704100000.000100"]
    );
}

#[test]
fn member_counts_deduplicates_within_chunk_and_across_chunk_boundary() {
    let mut store = Store::open_in_memory().unwrap();
    let mk_user = |id: &str, is_bot: bool, deleted: bool| User {
        id: UserId::from(id),
        is_bot,
        deleted,
        ..Default::default()
    };

    // Seed 520 unique users so members straddle SQL_IN_CHUNK (500):
    // - U0000..U0499: 450 active humans, 30 active bots, 20 deleted humans
    // - U0500..U0519: 10 active humans, 5 active bots, 5 deleted bots
    let mut users = Vec::new();
    for i in 0..500 {
        let (is_bot, deleted) = if i < 30 {
            (true, false)
        } else if i < 50 {
            (false, true)
        } else {
            (false, false)
        };
        users.push(mk_user(&format!("U{i:04}"), is_bot, deleted));
    }
    for i in 500..520 {
        let (is_bot, deleted) = if i < 505 {
            (true, false)
        } else if i < 510 {
            (true, true)
        } else {
            (false, false)
        };
        users.push(mk_user(&format!("U{i:04}"), is_bot, deleted));
    }
    store.upsert_users(&ws(), &users).unwrap();

    // 1. Within a single chunk: heavy duplication of human, bot, deleted, unknown, USLACKBOT.
    let single_chunk = [
        "U0000",
        "U0000",
        "U0000", // active bot x3
        "U0030",
        "U0030", // deleted human x2
        "U0505",
        "U0505", // deleted bot x2
        "U0100",
        "U0100", // active human x2
        "UUNKNOWN",
        "UUNKNOWN", // unknown human x2
        "USLACKBOT",
        "USLACKBOT", // built-in bot x2
    ]
    .map(UserId::from)
    .to_vec();
    assert_eq!(
        store.member_counts(&ws(), &single_chunk).unwrap(),
        MemberCounts { humans: 2, apps: 1 }
    );

    // 2. Across the SQL_IN_CHUNK = 500 boundary: include all 520 users + 3 unknown humans +
    //    USLACKBOT, PLUS duplicates of bots, deleted users, humans, and unknowns repeated at the
    //    beginning and end so the same IDs would fall into multiple chunks without deduplication.
    // Prepend duplicates of chunk-2 users (bots & deleted) into chunk 1.
    let mut big: Vec<UserId> = vec![
        UserId::from("U0500"),
        UserId::from("U0505"),
        UserId::from("U0000"),
        UserId::from("USLACKBOT"),
    ];
    for i in 0..520 {
        big.push(UserId::from(format!("U{i:04}")));
    }
    for u in ["UEXT1", "UEXT2", "UEXT3", "UEXT1"] {
        big.push(UserId::from(u));
    }
    // Append duplicates of chunk-1 bots/deleted/humans into chunk 2.
    for dup in [
        "U0000",
        "U0001",
        "U0030",
        "U0031",
        "U0100",
        "U0500",
        "U0505",
        "USLACKBOT",
    ] {
        big.push(UserId::from(dup));
    }
    assert!(big.len() > SQL_IN_CHUNK);

    // Expected unique totals:
    // - active humans: 450 + 10 + 3 unknown = 463
    // - active apps: 30 + 5 = 35
    // - deleted (20 humans + 5 bots) and USLACKBOT excluded
    assert_eq!(
        store.member_counts(&ws(), &big).unwrap(),
        MemberCounts {
            humans: 463,
            apps: 35,
        }
    );
}

#[test]
fn record_deletion_covers_compressed_unseen_edited_and_idempotency() {
    let mut store = Store::open_in_memory().unwrap();

    // 1. Deletion on a compressed BLOB message preserves decoded content without `previous`.
    let long_text = "Compressed body that survives deletion ".repeat(20);
    let compressed = msg("1700000100.000100", "U1", &long_text);
    store
        .upsert_message(&ws(), &general(), &compressed)
        .unwrap();
    let stored_type: String = store
        .conn
        .query_row(
            "SELECT typeof(json) FROM message_current WHERE ts = '1700000100.000100'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored_type, "blob");

    store
        .record_deletion(
            &ws(),
            &general(),
            &compressed.ts,
            Some(&Ts::from("1700000100.000900")),
            None,
        )
        .unwrap();
    let cur = store
        .message(&ws(), &general(), &compressed.ts)
        .unwrap()
        .unwrap();
    assert!(cur.deleted);
    assert_eq!(cur.message.text, long_text);
    let revs = store.revisions(&ws(), &general(), &compressed.ts).unwrap();
    assert_eq!(revs.len(), 2);
    assert!(!revs[0].deleted);
    assert_eq!(revs[0].message.text, long_text);
    assert!(revs[1].deleted);
    assert_eq!(revs[1].message.text, long_text);

    // 2. Deletion on an unseen message with `previous: Some`.
    let unseen_prev = msg("1700000200.000100", "U2", "never seen before delete");
    store
        .record_deletion(
            &ws(),
            &general(),
            &unseen_prev.ts,
            Some(&Ts::from("1700000200.000500")),
            Some(&unseen_prev),
        )
        .unwrap();
    let cur = store
        .message(&ws(), &general(), &unseen_prev.ts)
        .unwrap()
        .unwrap();
    assert!(cur.deleted);
    assert_eq!(cur.message.text, "never seen before delete");
    let revs = store.revisions(&ws(), &general(), &unseen_prev.ts).unwrap();
    assert_eq!(revs.len(), 1);
    assert!(revs[0].deleted);

    // 3. Deletion on an unseen message with `previous: None`.
    let ghost_ts = Ts::from("1700000300.000100");
    store
        .record_deletion(
            &ws(),
            &general(),
            &ghost_ts,
            Some(&Ts::from("1700000300.000500")),
            None,
        )
        .unwrap();
    let ghost = store
        .message(&ws(), &general(), &ghost_ts)
        .unwrap()
        .unwrap();
    assert!(ghost.deleted);
    assert_eq!(ghost.message.ts, ghost_ts);
    assert!(ghost.message.text.is_empty());
    assert_eq!(
        store.revisions(&ws(), &general(), &ghost_ts).unwrap().len(),
        1
    );

    // 4. Deletion on an already-edited message produces 3 revisions, and duplicate deletion with
    //    the same event_ts is idempotent.
    let v1 = msg("1700000400.000100", "U1", "draft v1");
    let mut v2 = msg("1700000400.000100", "U1", "edited v2");
    v2.edited = Some(Edited {
        user: Some(UserId::from("U1")),
        ts: Ts::from("1700000400.000200"),
    });
    store.upsert_message(&ws(), &general(), &v1).unwrap();
    store.upsert_message(&ws(), &general(), &v2).unwrap();
    let del_ev = Ts::from("1700000400.000300");
    store
        .record_deletion(&ws(), &general(), &v1.ts, Some(&del_ev), None)
        .unwrap();
    // Duplicate deletion event is idempotent.
    store
        .record_deletion(&ws(), &general(), &v1.ts, Some(&del_ev), None)
        .unwrap();

    let revs = store.revisions(&ws(), &general(), &v1.ts).unwrap();
    assert_eq!(revs.len(), 3);
    assert_eq!(
        revs.iter()
            .map(|r| (r.edit_ts.as_str(), r.deleted, r.message.text.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("", false, "draft v1"),
            ("1700000400.000200", false, "edited v2"),
            ("1700000400.000300", true, "edited v2"),
        ]
    );
}

#[test]
fn thread_broadcast_appears_in_channel_history_and_latest_messages() {
    let mut store = Store::open_in_memory().unwrap();
    let root = msg("1700001000.000100", "U1", "thread root");
    let mut normal_reply = msg("1700001001.000100", "U2", "regular reply");
    normal_reply.thread_ts = Some(root.ts.clone());
    let mut broadcast_reply = msg("1700001002.000100", "U2", "also sent to channel");
    broadcast_reply.thread_ts = Some(root.ts.clone());
    broadcast_reply.subtype = Some(MessageSubtype::ThreadBroadcast);
    let mut later_reply = msg("1700001003.000100", "U1", "later regular reply");
    later_reply.thread_ts = Some(root.ts.clone());

    store
        .upsert_messages(
            &ws(),
            &general(),
            &[root.clone(), normal_reply, broadcast_reply, later_reply],
        )
        .unwrap();

    let history = store.channel_history(&ws(), &general(), None, 20).unwrap();
    let hist_texts: Vec<&str> = history.iter().map(|m| m.message.text.as_str()).collect();
    assert_eq!(hist_texts, ["thread root", "also sent to channel"]);

    let latest = store.latest_messages(&ws(), &[general()]).unwrap();
    assert_eq!(latest[&general()].message.text, "also sent to channel");

    let thread = store.thread_replies(&ws(), &general(), &root.ts).unwrap();
    assert_eq!(thread.len(), 4);
}

#[test]
fn same_edit_ts_upsert_updates_reactions_reply_count_and_pinned_without_duplicate_revisions() {
    let mut store = Store::open_in_memory().unwrap();
    let mut m = msg("1700002000.000100", "U1", "status update");
    m.reactions = vec![Reaction {
        name: "thumbsup".into(),
        count: 2,
        users: vec![UserId::from("U1"), UserId::from("U2")],
    }];
    m.reply_count = Some(1);
    m.pinned_to = vec![general()];
    store.upsert_message(&ws(), &general(), &m).unwrap();

    let rev_table_count = |store: &Store| -> i64 {
        store
            .conn
            .query_row("SELECT count(*) FROM message_revision", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(rev_table_count(&store), 0);
    assert_eq!(
        store
            .search(&ws(), &SearchQuery::parse("has:reaction"), 10)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .search(&ws(), &SearchQuery::parse("has:pin"), 10)
            .unwrap()
            .len(),
        1
    );

    // Re-upsert with the same edit_ts (""): clear reactions, bump reply_count, unpin.
    m.reactions.clear();
    m.reply_count = Some(5);
    m.pinned_to.clear();
    store.upsert_message(&ws(), &general(), &m).unwrap();

    assert_eq!(rev_table_count(&store), 0);
    assert_eq!(store.revisions(&ws(), &general(), &m.ts).unwrap().len(), 1);
    let stored = store.message(&ws(), &general(), &m.ts).unwrap().unwrap();
    assert!(stored.message.reactions.is_empty());
    assert_eq!(stored.message.reply_count, Some(5));
    assert!(stored.message.pinned_to.is_empty());
    assert!(
        store
            .search(&ws(), &SearchQuery::parse("has:reaction"), 10)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .search(&ws(), &SearchQuery::parse("has:pin"), 10)
            .unwrap()
            .is_empty()
    );

    // Edit once (now 2 revisions), then update metadata again with the same non-empty edit_ts.
    m.text = "status update edited".into();
    m.edited = Some(Edited {
        user: Some(UserId::from("U1")),
        ts: Ts::from("1700002000.000500"),
    });
    store.upsert_message(&ws(), &general(), &m).unwrap();
    assert_eq!(rev_table_count(&store), 2);

    m.reply_count = Some(9);
    m.pinned_to = vec![general()];
    store.upsert_message(&ws(), &general(), &m).unwrap();
    assert_eq!(rev_table_count(&store), 2);
    let stored = store.message(&ws(), &general(), &m.ts).unwrap().unwrap();
    assert_eq!(stored.message.reply_count, Some(9));
    assert_eq!(stored.message.pinned_to, vec![general()]);
}

#[test]
fn ts_key_handles_malformed_and_utf8_fractional_parts() {
    // Non-ASCII character straddling byte index 6 must not panic.
    assert_eq!(
        ts_key(&Ts::from("1700000000.12345é")),
        1_700_000_000_000_000
    );
    assert_eq!(ts_key(&Ts::from("1700000000.🚀🎉")), 1_700_000_000_000_000);
    assert_eq!(
        ts_key(&Ts::from("1700000000.12🚀345")),
        1_700_000_000_000_000
    );
    assert_eq!(
        ts_key(&Ts::from("1700000000.123456789")),
        1_700_000_000_123_456
    );
    assert_eq!(ts_key(&Ts::from("1700000000.-1")), 1_700_000_000_000_000);
    assert_eq!(ts_key(&Ts::from("not-a-ts.abc")), 0);
    assert_eq!(ts_key(&Ts::from("not-a-ts.123456")), 0);
    assert_eq!(ts_key(&Ts::from("")), 0);
    assert_eq!(parse_ts_key(&Ts::from("not-a-ts.123456")), None);
    assert_eq!(parse_ts_key(&Ts::from("-1700000000.000100")), None);
    assert_eq!(
        parse_ts_key(&Ts::from("1700000000.000100")),
        Some(1_700_000_000_000_100)
    );
}

#[test]
fn display_order_handles_cycles_self_loops_and_dangling_next_id() {
    let sec = |id: &str, next: Option<&str>| ChannelSection {
        id: id.to_owned(),
        name: id.to_owned(),
        kind: SectionKind::Standard,
        emoji: String::new(),
        next_id: next.map(str::to_owned),
        channel_ids: Vec::new(),
    };
    let ids = |sections: Vec<ChannelSection>| -> Vec<String> {
        sections.into_iter().map(|s| s.id).collect()
    };

    // Full cycle (a -> b -> a): no unreferenced head -> falls back to stored order.
    assert_eq!(
        ids(display_order(vec![
            sec("a", Some("b")),
            sec("b", Some("a")),
        ])),
        vec!["a", "b"]
    );

    // Partial cycle (a -> b -> c -> b) with unreached leftover d: head is a, stops at cycle guard.
    assert_eq!(
        ids(display_order(vec![
            sec("c", Some("b")),
            sec("d", Some("d")),
            sec("a", Some("b")),
            sec("b", Some("c")),
        ])),
        vec!["a", "b", "c", "d"]
    );

    // Standalone self-loop (a -> a): every section is referenced -> stored order.
    assert_eq!(ids(display_order(vec![sec("a", Some("a"))])), vec!["a"]);

    // Self-loop alongside an unreferenced head (b -> None, a -> a).
    assert_eq!(
        ids(display_order(vec![sec("a", Some("a")), sec("b", None)])),
        vec!["b", "a"]
    );

    // Dangling next_id (a -> missing): follows a, stops at missing, appends leftover b.
    assert_eq!(
        ids(display_order(vec![
            sec("a", Some("missing")),
            sec("b", None)
        ])),
        vec!["a", "b"]
    );
}

#[test]
fn bot_profiles_round_trip_workspace_isolation_and_corrupt_json_tolerance() {
    let mut store = Store::open_in_memory().unwrap();
    let bot_a = BotProfile {
        id: BotId::from("B1"),
        app_id: Some("A1".into()),
        name: "DeployBot".into(),
        icons: Some(BotIcons {
            image_48: Some("https://img.example/bot.png".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let bot_b = BotProfile {
        id: BotId::from("B2"),
        name: "OtherWsBot".into(),
        ..Default::default()
    };

    store
        .upsert_bot_profile(&ws(), &BotId::from("B1"), &bot_a)
        .unwrap();
    store
        .upsert_bot_profile(&TeamId::from("T2"), &BotId::from("B2"), &bot_b)
        .unwrap();

    // Update existing profile in place.
    let mut updated_a = bot_a.clone();
    updated_a.name = "DeployBot v2".into();
    store
        .upsert_bot_profile(&ws(), &BotId::from("B1"), &updated_a)
        .unwrap();

    // Insert a corrupt JSON row directly; bot_profiles() must skip it without failing.
    store
        .conn
        .execute(
            "INSERT INTO bot_profile (workspace_id, bot_id, json, updated_at)
             VALUES ('T1', 'BCORRUPT', '{not-valid-json', 100)",
            [],
        )
        .unwrap();

    let t1_bots = store.bot_profiles(&ws()).unwrap();
    assert_eq!(t1_bots.len(), 1);
    assert_eq!(t1_bots[&BotId::from("B1")].name, "DeployBot v2");

    let t2_bots = store.bot_profiles(&TeamId::from("T2")).unwrap();
    assert_eq!(t2_bots.len(), 1);
    assert_eq!(t2_bots[&BotId::from("B2")].name, "OtherWsBot");
}

#[test]
fn merge_conversation_inserts_new_and_overlays_preserving_stored_fields() {
    let mut store = Store::open_in_memory().unwrap();

    // New insert when conversation is absent.
    let initial = Conversation {
        id: general(),
        name: Some("general".into()),
        is_channel: true,
        is_member: true,
        unread_count: Some(7),
        topic: Some(TopicPurpose {
            value: "Original topic".into(),
            ..Default::default()
        }),
        members: vec![UserId::from("U1"), UserId::from("U2")],
        ..Default::default()
    };
    store.merge_conversation(&ws(), &initial).unwrap();
    let stored = store.conversation(&ws(), &general()).unwrap().unwrap();
    assert_eq!(stored, initial);

    // Partial overlay from conversations.info: carries num_members + new name, omits
    // is_member/unread_count/topic/members (which must be preserved).
    let fresh = Conversation {
        id: general(),
        name: Some("general-renamed".into()),
        num_members: Some(42),
        purpose: Some(TopicPurpose {
            value: "Company announcements".into(),
            ..Default::default()
        }),
        ..Default::default()
    };
    store.merge_conversation(&ws(), &fresh).unwrap();
    let merged = store.conversation(&ws(), &general()).unwrap().unwrap();
    assert_eq!(merged.name.as_deref(), Some("general-renamed"));
    assert_eq!(merged.num_members, Some(42));
    assert_eq!(
        merged.purpose.as_ref().map(|p| p.value.as_str()),
        Some("Company announcements")
    );
    assert!(merged.is_member, "omitted false flag must not erase true");
    assert_eq!(merged.unread_count, Some(7));
    assert_eq!(
        merged.topic.as_ref().map(|t| t.value.as_str()),
        Some("Original topic")
    );
    assert_eq!(merged.members, vec![UserId::from("U1"), UserId::from("U2")]);
}

#[test]
fn open_read_only_rejects_writes_and_open_rejects_future_schema() {
    let dir = std::env::temp_dir().join(format!(
        "slackless-store-ro-writes-test-{}",
        std::process::id()
    ));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("readonly.sqlite");
    let _ = std::fs::remove_file(&path);

    let mut writer = Store::open(&path).unwrap();
    writer.set_local(&ws(), "theme", "dark").unwrap();
    writer
        .upsert_message(&ws(), &general(), &msg("1700000000.000100", "U1", "hello"))
        .unwrap();

    let mut reader = Store::open_read_only(&path).unwrap();
    assert_eq!(
        reader.local(&ws(), "theme").unwrap().as_deref(),
        Some("dark")
    );
    // Multi-query read transactions must work cleanly on read-only handles.
    assert_eq!(reader.stats(&ws()).unwrap().messages, 1);
    assert_eq!(
        reader
            .storage_breakdown(&ws(), 5)
            .unwrap()
            .top_channels
            .len(),
        1
    );
    assert_eq!(
        reader.latest_messages(&ws(), &[general()]).unwrap().len(),
        1
    );
    assert_eq!(reader.digest_since(&ws(), 0, 10).unwrap().len(), 1);

    assert!(matches!(
        reader.set_local(&ws(), "theme", "light"),
        Err(StoreError::ReadOnly)
    ));
    assert!(matches!(
        reader.upsert_message(&ws(), &general(), &msg("1700000001.000100", "U1", "nope")),
        Err(StoreError::ReadOnly)
    ));
    assert!(matches!(
        reader.enqueue_offline(&ws(), "{}"),
        Err(StoreError::ReadOnly)
    ));
    assert!(matches!(reader.vacuum(), Err(StoreError::ReadOnly)));
    assert!(matches!(
        reader.delete_huddle_transcript(&ws(), "R0CALL"),
        Err(StoreError::ReadOnly)
    ));

    drop(reader);

    // Bump user_version past SCHEMA_VERSION and verify both Store::open and Store::open_read_only
    // return FutureSchema.
    let future = schema::SCHEMA_VERSION + 2;
    writer
        .conn
        .pragma_update(None, "user_version", future)
        .unwrap();
    drop(writer);

    for res in [Store::open(&path), Store::open_read_only(&path)] {
        let err = match res {
            Ok(_) => panic!("expected FutureSchema error"),
            Err(err) => err,
        };
        assert!(
            matches!(
                err,
                StoreError::FutureSchema {
                    found,
                    supported,
                } if found == future && supported == schema::SCHEMA_VERSION
            ),
            "expected FutureSchema, got {err:?}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn archive_read_apis_cover_channels_dms_users_resolve_digest_and_malformed_prefs() {
    use crate::types::Latest;

    let mut store = Store::open_in_memory().unwrap();
    let ws1 = TeamId::from("T1");
    let ws2 = TeamId::from("T2");

    let mk_user =
        |id: &str, name: &str, display: Option<&str>, real: Option<&str>, bot: bool, del: bool| {
            User {
                id: UserId::from(id),
                name: name.into(),
                real_name: real.map(str::to_owned),
                profile: Profile {
                    display_name: display.map(str::to_owned),
                    real_name: real.map(str::to_owned),
                    ..Default::default()
                },
                is_bot: bot,
                deleted: del,
                ..Default::default()
            }
        };

    store
        .upsert_users(
            &ws1,
            &[
                mk_user(
                    "U1",
                    "alice",
                    Some("Alice Display"),
                    Some("Alice Real"),
                    false,
                    false,
                ),
                mk_user("U2", "bob", Some(""), Some("Bob Real"), false, false),
                mk_user("U3", "carol", Some(""), Some(""), false, false),
                mk_user("U4", "zbot", Some("Zeta Bot"), None, true, false),
                mk_user("U5", "former", Some("Former Member"), None, false, true),
            ],
        )
        .unwrap();

    store
        .upsert_conversations(
            &ws1,
            &[
                Conversation {
                    id: ChannelId::from("C_ENG"),
                    name: Some("Engineering".into()),
                    is_channel: true,
                    is_member: true,
                    topic: Some(TopicPurpose {
                        value: "Build".into(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                Conversation {
                    id: ChannelId::from("C_ALPHA"),
                    name: Some("alpha".into()),
                    is_channel: true,
                    is_private: true,
                    is_archived: true,
                    is_member: false,
                    ..Default::default()
                },
                Conversation {
                    id: ChannelId::from("C_UNNAMED"),
                    name: None,
                    is_channel: true,
                    is_member: true,
                    ..Default::default()
                },
                Conversation {
                    id: ChannelId::from("C_BOB"),
                    name: Some("bob".into()),
                    is_channel: true,
                    is_member: true,
                    ..Default::default()
                },
                Conversation {
                    id: ChannelId::from("D_ALICE"),
                    is_im: true,
                    user: Some(UserId::from("U1")),
                    latest: Some(Latest::Ts(Ts::from("1700000300.000000"))),
                    ..Default::default()
                },
                Conversation {
                    id: ChannelId::from("D_BOB"),
                    is_im: true,
                    user: Some(UserId::from("U2")),
                    latest: None,
                    ..Default::default()
                },
                Conversation {
                    id: ChannelId::from("G_MPIM"),
                    name: Some("mpdm-a--b".into()),
                    is_mpim: true,
                    latest: Some(Latest::Ts(Ts::from("1700000500.000000"))),
                    ..Default::default()
                },
            ],
        )
        .unwrap();

    // Channels sorted case-insensitively with unnamed last.
    let channels = store.list_channels(&ws1).unwrap();
    let ch_ids: Vec<&str> = channels.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ch_ids, ["C_ALPHA", "C_BOB", "C_ENG", "C_UNNAMED"]);
    assert_eq!(channels[0].label(), "#alpha");
    assert_eq!(channels[2].topic.as_deref(), Some("Build"));
    assert_eq!(channels[3].label(), "C_UNNAMED");

    // DMs sorted by latest_secs desc with None last.
    let dms = store.list_dms(&ws1).unwrap();
    let dm_ids: Vec<&str> = dms.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(dm_ids, ["G_MPIM", "D_ALICE", "D_BOB"]);
    assert_eq!(dms[0].latest_secs, Some(1_700_000_500));
    assert_eq!(dms[2].latest_secs, None);

    // channel_names omits unnamed conversations.
    let names = store.channel_names(&ws1).unwrap();
    assert_eq!(names.get("C_ENG").map(String::as_str), Some("Engineering"));
    assert!(!names.contains_key("C_UNNAMED"));

    // conversation_summary direct PK lookup.
    assert_eq!(
        store
            .conversation_summary(&ws1, &ChannelId::from("C_ENG"))
            .unwrap()
            .unwrap()
            .name
            .as_deref(),
        Some("Engineering")
    );
    assert!(
        store
            .conversation_summary(&ws1, &ChannelId::from("C_NOPE"))
            .unwrap()
            .is_none()
    );

    // resolve_channel precedence: exact id -> case-insensitive name -> IM by user handle/display/real.
    assert_eq!(
        store.resolve_channel(&ws1, "C_ENG").unwrap().map(|c| c.id),
        Some(ChannelId::from("C_ENG"))
    );
    assert_eq!(
        store
            .resolve_channel(&ws1, "#engineering")
            .unwrap()
            .map(|c| c.id),
        Some(ChannelId::from("C_ENG"))
    );
    assert_eq!(
        store.resolve_channel(&ws1, "bob").unwrap().map(|c| c.id),
        Some(ChannelId::from("C_BOB"))
    );
    assert_eq!(
        store.resolve_channel(&ws1, "@alice").unwrap().map(|c| c.id),
        Some(ChannelId::from("D_ALICE"))
    );
    assert_eq!(
        store
            .resolve_channel(&ws1, "Bob Real")
            .unwrap()
            .map(|c| c.id),
        Some(ChannelId::from("D_BOB"))
    );
    assert!(store.resolve_channel(&ws1, "#missing").unwrap().is_none());

    // user_display & list_users.
    assert_eq!(
        store
            .user_display(&ws1, &UserId::from("U1"))
            .unwrap()
            .as_deref(),
        Some("Alice Display")
    );
    assert_eq!(
        store
            .user_display(&ws1, &UserId::from("U2"))
            .unwrap()
            .as_deref(),
        Some("Bob Real")
    );
    assert_eq!(
        store
            .user_display(&ws1, &UserId::from("U3"))
            .unwrap()
            .as_deref(),
        Some("carol")
    );
    assert_eq!(
        store.user_display(&ws1, &UserId::from("U99")).unwrap(),
        None
    );

    let active_users: Vec<String> = store
        .list_users(&ws1, None, false)
        .unwrap()
        .into_iter()
        .map(|u| u.id.as_str().to_owned())
        .collect();
    assert_eq!(active_users, ["U1", "U2", "U3", "U4"]);
    let all_users: Vec<String> = store
        .list_users(&ws1, None, true)
        .unwrap()
        .into_iter()
        .map(|u| u.id.as_str().to_owned())
        .collect();
    assert_eq!(all_users, ["U1", "U2", "U3", "U5", "U4"]);
    assert_eq!(
        store.list_users(&ws1, Some("@alice"), false).unwrap().len(),
        1
    );

    // Messages, latest_ts (skips deleted), message_count, workspace isolation, digest_since.
    store
        .upsert_messages(
            &ws1,
            &ChannelId::from("C_ENG"),
            &[
                msg("1700000050.000000", "U1", "before cutoff"),
                msg("1700000100.000000", "U1", "at cutoff"),
                msg("1700000200.000000", "U2", "second author"),
                msg("1700000250.000000", "U1", "to be deleted"),
            ],
        )
        .unwrap();
    store
        .record_deletion(
            &ws1,
            &ChannelId::from("C_ENG"),
            &Ts::from("1700000250.000000"),
            Some(&Ts::from("1700000251.000000")),
            None,
        )
        .unwrap();
    store
        .upsert_messages(
            &ws1,
            &ChannelId::from("D_ALICE"),
            &[
                msg("1700000150.000000", "U1", "dm one"),
                msg(
                    "1700000300.000000",
                    "U1",
                    &format!("hi\t\n{}", "é".repeat(130)),
                ),
            ],
        )
        .unwrap();
    store
        .upsert_message(
            &ws2,
            &ChannelId::from("C_OTHER"),
            &msg("1700000999.000000", "U1", "other"),
        )
        .unwrap();

    assert_eq!(store.message_count(&ws1).unwrap(), 6);
    assert_eq!(store.message_count(&ws2).unwrap(), 1);
    assert_eq!(
        store.latest_ts(&ws1, &ChannelId::from("C_ENG")).unwrap(),
        Some(Ts::from("1700000200.000000"))
    );
    assert_eq!(
        store.latest_ts(&ws1, &ChannelId::from("C_ALPHA")).unwrap(),
        None
    );

    let digest = store.digest_since(&ws1, 1_700_000_100, 10).unwrap();
    assert_eq!(digest.len(), 2);
    assert_eq!(digest[0].channel, ChannelId::from("D_ALICE"));
    assert_eq!(digest[0].label, "@Alice Display");
    assert_eq!(digest[0].message_count, 2);
    assert_eq!(digest[0].latest_text.chars().count(), 120);
    assert!(digest[0].latest_text.ends_with('…'));
    assert_eq!(digest[1].channel, ChannelId::from("C_ENG"));
    assert_eq!(digest[1].label, "#Engineering");
    assert_eq!(digest[1].author_count, 2);

    // Malformed prefs JSON in SQLite returns StoreError::Json from Store::prefs.
    store
        .conn
        .execute(
            "INSERT OR REPLACE INTO prefs (workspace_id, json, updated_at) VALUES (?1, ?2, 0)",
            params!["T_BAD", "{not valid json"],
        )
        .unwrap();
    assert!(matches!(
        store.prefs(&TeamId::from("T_BAD")),
        Err(StoreError::Json(_))
    ));
}

#[test]
fn search_date_filters_enforce_exact_utc_midnight_boundaries_leap_days_and_rollover() {
    let mut store = Store::open_in_memory().unwrap();
    let ch = general();

    // Seed messages at exact UTC boundaries:
    // - 2024-02-28T23:59:59Z = 1709164799
    // - 2024-02-29T00:00:00Z = 1709164800 (leap day start)
    // - 2024-02-29T23:59:59Z = 1709251199 (leap day end)
    // - 2024-03-01T00:00:00Z = 1709251200
    // - 2025-12-31T23:59:59Z = 1767225599 (year rollover before midnight)
    // - 2026-01-01T00:00:00Z = 1767225600 (year rollover midnight)
    // - 2026-01-01T23:59:59Z = 1767311999 (exact second before 2026-01-02)
    // - 2026-01-02T00:00:00Z = 1767312000 (exact midnight of 2026-01-02)
    // - 2026-01-02T23:59:59Z = 1767398399
    // - 2026-01-03T00:00:00Z = 1767398400
    let fixtures = [
        ("1709164799.999999", "feb28-end"),
        ("1709164800.000000", "leap-start"),
        ("1709251199.999999", "leap-end"),
        ("1709251200.000000", "mar01-start"),
        ("1767225599.999999", "y2025-end"),
        ("1767225600.000000", "y2026-start"),
        ("1767311999.999999", "jan01-end"),
        ("1767312000.000000", "jan02-start"),
        ("1767398399.999999", "jan02-end"),
        ("1767398400.000000", "jan03-start"),
    ];
    let msgs: Vec<Message> = fixtures
        .iter()
        .map(|(ts, tag)| msg(ts, "U1", &format!("marker {tag}")))
        .collect();
    store.upsert_messages(&ws(), &ch, &msgs).unwrap();

    let tags = |q: &str| -> Vec<String> {
        store
            .search(&ws(), &SearchQuery::parse(q), 50)
            .unwrap()
            .into_iter()
            .map(|m| m.message.text.trim_start_matches("marker ").to_owned())
            .collect()
    };

    // Exact midnight exclusivity for `before:2026-01-02`:
    // includes 2026-01-01T23:59:59Z, excludes 2026-01-02T00:00:00Z.
    let before_jan02 = tags("marker before:2026-01-02");
    assert!(before_jan02.contains(&"jan01-end".to_owned()));
    assert!(!before_jan02.contains(&"jan02-start".to_owned()));
    assert_eq!(before_jan02.len(), 7);

    // Exact midnight exclusivity for `after:2026-01-01`:
    // excludes 2026-01-01T23:59:59Z, includes 2026-01-02T00:00:00Z.
    let after_jan01 = tags("marker after:2026-01-01");
    assert_eq!(after_jan01, vec!["jan03-start", "jan02-end", "jan02-start"]);

    // Combined range isolating exactly one calendar day (2026-01-02).
    assert_eq!(
        tags("marker after:2026-01-01 before:2026-01-03"),
        vec!["jan02-end", "jan02-start"]
    );

    // Adjacent exclusive bounds (`after:2026-01-01 before:2026-01-02`) yield empty.
    assert!(tags("marker after:2026-01-01 before:2026-01-02").is_empty());

    // Leap day isolation: strictly after 2024-02-28 and strictly before 2024-03-01.
    assert_eq!(
        tags("marker after:2024-02-28 before:2024-03-01"),
        vec!["leap-end", "leap-start"]
    );
    // `before:2024-02-29` excludes leap-start, keeps feb28-end.
    assert_eq!(tags("marker before:2024-02-29"), vec!["feb28-end"]);
    // `after:2024-02-29 before:2024-03-02` includes mar01-start, excludes leap-end.
    assert_eq!(
        tags("marker after:2024-02-29 before:2024-03-02"),
        vec!["mar01-start"]
    );

    // Year rollover isolation: strictly after 2025-12-31 and before 2026-01-02 => 2026-01-01 only.
    assert_eq!(
        tags("marker after:2025-12-31 before:2026-01-02"),
        vec!["jan01-end", "y2026-start"]
    );
    // Strictly before 2026-01-01 includes 2025-12-31T23:59:59Z, excludes 2026-01-01T00:00:00Z.
    let before_2026 = tags("marker before:2026-01-01");
    assert_eq!(before_2026.first().map(String::as_str), Some("y2025-end"));
    assert!(!before_2026.contains(&"y2026-start".to_owned()));

    // Invalid dates (non-leap Feb 29, Feb 31, month 0/13) do not parse as filters and stay free text.
    for bad in [
        "2025-02-29",
        "1900-02-29",
        "2026-02-31",
        "2026-13-01",
        "2026-00-10",
    ] {
        let q = SearchQuery::parse(&format!("marker before:{bad}"));
        assert_eq!(q.filters.before, None);
        assert!(store.search(&ws(), &q, 10).unwrap().is_empty());
    }
}

#[test]
#[ignore = "benchmark; run with --release -- --ignored --nocapture"]
fn bench_ingest_and_fts_10k_messages() {
    use std::time::Instant;

    let mut store = Store::open_in_memory().unwrap();
    let workspace = TeamId::from("TBENCH");

    let channel_names = [
        ("C00", "general"),
        ("C01", "engineering"),
        ("C02", "ops"),
        ("C03", "deploys"),
        ("C04", "design"),
        ("C05", "alerts"),
        ("C06", "security"),
        ("C07", "backend"),
        ("C08", "frontend"),
        ("C09", "random"),
    ];
    let conversations: Vec<Conversation> = channel_names
        .iter()
        .map(|(id, name)| Conversation {
            id: ChannelId::from(*id),
            name: Some((*name).to_owned()),
            is_channel: true,
            is_member: true,
            ..Default::default()
        })
        .collect();
    store
        .upsert_conversations(&workspace, &conversations)
        .unwrap();

    let users: Vec<User> = (0..20)
        .map(|i| User {
            id: UserId::from(format!("U{i:02}")),
            name: format!("user{i:02}"),
            real_name: Some(format!("Engineer {i:02}")),
            profile: Profile {
                display_name: Some(format!("eng{i:02}")),
                ..Default::default()
            },
            ..Default::default()
        })
        .collect();
    store.upsert_users(&workspace, &users).unwrap();

    let topics = [
        "kubernetes cluster autoscaler tuned for peak latency",
        "postgres vacuum completed on shard primary replica",
        "sqlite fts5 index rebuild finished without regressions",
        "canary deploy rolled out cleanly across us-east nodes",
        "grafana dashboard updated with p99 checkout timings",
        "webrtc audio jitter buffer calibrated for huddles",
        "auth keychain migration verified on macos arm64",
        "ci pipeline cache hit rate climbed to ninety percent",
        "tls certificate rotation scheduled ahead of expiry",
        "incident postmortem action items assigned to owners",
    ];

    // Base timestamp: 2026-01-01T00:00:00Z = 1_767_225_600.
    // 10,000 messages spaced 300s apart (~34.7 days: 2026-01-01 through 2026-02-04).
    const TOTAL_MESSAGES: usize = 10_000;
    const BASE_SECS: i64 = 1_767_225_600;
    const STEP_SECS: i64 = 300;
    const BATCH_SIZE: usize = 200;

    let mut per_channel: Vec<(ChannelId, Vec<Message>)> = channel_names
        .iter()
        .map(|(id, _)| {
            (
                ChannelId::from(*id),
                Vec::with_capacity(TOTAL_MESSAGES / 10),
            )
        })
        .collect();

    let mut expected_ops_u03_postmortem = 0usize;
    let mut expected_dense_ts = String::new();
    let mut expected_sparse_newer_ts = String::new();

    for i in 0..TOTAL_MESSAGES {
        let ch_idx = i % channel_names.len();
        let user_idx = (i / channel_names.len() + ch_idx) % 20;
        let secs = BASE_SECS + (i as i64) * STEP_SECS;
        let ts_str = format!("{secs}.{:06}", i % 1_000_000);
        let topic = topics[i % topics.len()];

        // Inject deterministic benchmark needles for exact accuracy checks:
        // 1. Dense BM25 needle vs newer sparse needle for "hypernova".
        let text = if i == 1_234 {
            expected_dense_ts = ts_str.clone();
            "hypernova hypernova hypernova cluster rollout hypernova checklist".to_owned()
        } else if i == 9_876 {
            expected_sparse_newer_ts = ts_str.clone();
            "quick mention of hypernova in weekly sync".to_owned()
        } else if ch_idx == 2 && user_idx == 3 && (i / 200) % 2 == 0 {
            // In #ops (C02) by U03 (user03) with unique needle (25 deterministic matches).
            expected_ops_u03_postmortem += 1;
            format!("ops_postmortem_needle #{i}: {topic} https://status.example/{i}")
        } else if i.is_multiple_of(17) {
            format!("msg #{i}: {topic} https://docs.example/runbook/{i}")
        } else {
            format!("msg #{i}: {topic}")
        };

        let mut m = Message {
            ts: Ts::from(ts_str),
            user: Some(UserId::from(format!("U{user_idx:02}"))),
            text,
            ..Default::default()
        };
        if i.is_multiple_of(23) {
            m.files.push(File {
                id: FileId::from(format!("F{i:05}")),
                name: Some(format!("report-{i}.pdf")),
                ..Default::default()
            });
        }
        if i.is_multiple_of(37) {
            m.reactions.push(Reaction {
                name: "rocket".into(),
                count: 2,
                users: vec![UserId::from("U00"), UserId::from("U01")],
            });
        }
        per_channel[ch_idx].1.push(m);
    }

    let ingest_start = Instant::now();
    for (ch_id, msgs) in &per_channel {
        for chunk in msgs.chunks(BATCH_SIZE) {
            store.upsert_messages(&workspace, ch_id, chunk).unwrap();
        }
    }
    let ingest_elapsed = ingest_start.elapsed();
    let ingest_msgs_per_sec = TOTAL_MESSAGES as f64 / ingest_elapsed.as_secs_f64();

    assert_eq!(
        store.message_count(&workspace).unwrap(),
        TOTAL_MESSAGES as u64
    );
    assert!(expected_ops_u03_postmortem > 0);

    // Build 50 realistic FTS5 queries across Recent, BM25 Relevant, and structured modifiers.
    let queries: Vec<(&str, SearchSort, usize)> = vec![
        // Accuracy-checked benchmark queries first
        ("hypernova", SearchSort::Recent, 10),
        ("hypernova", SearchSort::Relevant, 10),
        (
            "ops_postmortem_needle in:#ops from:@user03",
            SearchSort::Recent,
            50,
        ),
        (
            "ops_postmortem_needle in:#ops from:@user03 has:link",
            SearchSort::Recent,
            50,
        ),
        (
            "kubernetes after:2026-01-05 before:2026-01-15",
            SearchSort::Recent,
            50,
        ),
        (
            "postgres in:#backend after:2026-01-10 before:2026-01-20",
            SearchSort::Recent,
            50,
        ),
        ("sqlite fts5 has:file", SearchSort::Recent, 50),
        (
            "canary deploy in:#deploys has:reaction",
            SearchSort::Recent,
            50,
        ),
        ("grafana checkout", SearchSort::Relevant, 25),
        ("webrtc huddles -staging", SearchSort::Recent, 25),
        // Remaining 40 varied queries to reach 50 total
        ("kubernetes", SearchSort::Recent, 20),
        ("kubernetes", SearchSort::Relevant, 20),
        ("postgres replica", SearchSort::Recent, 20),
        ("postgres replica", SearchSort::Relevant, 20),
        ("sqlite index", SearchSort::Recent, 20),
        ("sqlite index", SearchSort::Relevant, 20),
        ("canary deploy*", SearchSort::Recent, 20),
        ("grafana p99", SearchSort::Recent, 20),
        ("webrtc jitter", SearchSort::Relevant, 20),
        ("keychain arm64", SearchSort::Recent, 20),
        ("pipeline cache", SearchSort::Relevant, 20),
        ("certificate rotation", SearchSort::Recent, 20),
        ("postmortem owners", SearchSort::Relevant, 20),
        ("in:#general kubernetes", SearchSort::Recent, 20),
        ("in:#engineering postgres", SearchSort::Recent, 20),
        ("in:#ops sqlite", SearchSort::Recent, 20),
        ("in:#deploys canary", SearchSort::Recent, 20),
        ("in:#design grafana", SearchSort::Recent, 20),
        ("in:#alerts webrtc", SearchSort::Recent, 20),
        ("in:#security keychain", SearchSort::Recent, 20),
        ("in:#backend pipeline", SearchSort::Recent, 20),
        ("in:#frontend certificate", SearchSort::Recent, 20),
        ("in:#random postmortem", SearchSort::Recent, 20),
        ("from:@user00 latency", SearchSort::Recent, 20),
        ("from:@user05 vacuum", SearchSort::Recent, 20),
        ("from:@user10 rebuild", SearchSort::Recent, 20),
        ("from:@user15 rollout", SearchSort::Recent, 20),
        ("from:U01 has:file", SearchSort::Recent, 20),
        ("from:U02 has:link", SearchSort::Recent, 20),
        ("from:U04 has:reaction", SearchSort::Recent, 20),
        (
            "after:2026-01-01 before:2026-01-10 kubernetes",
            SearchSort::Recent,
            20,
        ),
        (
            "after:2026-01-10 before:2026-01-20 postgres",
            SearchSort::Recent,
            20,
        ),
        (
            "after:2026-01-20 before:2026-01-30 sqlite",
            SearchSort::Recent,
            20,
        ),
        (
            "after:2026-01-15 before:2026-02-01 canary",
            SearchSort::Relevant,
            20,
        ),
        (
            "in:#general in:#engineering latency",
            SearchSort::Recent,
            20,
        ),
        ("-in:#random kubernetes", SearchSort::Recent, 20),
        ("-from:@user00 postgres", SearchSort::Recent, 20),
        ("report-*.pdf", SearchSort::Recent, 20),
        ("\"cluster autoscaler\"", SearchSort::Recent, 20),
        (
            "latency -postgres in:#general after:2026-01-02 before:2026-02-01",
            SearchSort::Relevant,
            20,
        ),
    ];
    assert_eq!(queries.len(), 50);

    let mut latencies_us: Vec<u128> = Vec::with_capacity(queries.len());
    let search_start = Instant::now();

    for (idx, (raw_q, sort, limit)) in queries.iter().enumerate() {
        let parsed = SearchQuery::parse(raw_q);
        let q_start = Instant::now();
        let hits = store
            .search_sorted(&workspace, &parsed, *sort, *limit)
            .unwrap();
        latencies_us.push(q_start.elapsed().as_micros());

        match idx {
            0 => {
                // Recent sort returns newer sparse needle first.
                assert_eq!(hits.len(), 2);
                assert_eq!(hits[0].message.ts.as_str(), expected_sparse_newer_ts);
                assert_eq!(hits[1].message.ts.as_str(), expected_dense_ts);
            }
            1 => {
                // BM25 Relevant sort returns denser needle first.
                assert_eq!(hits.len(), 2);
                assert_eq!(hits[0].message.ts.as_str(), expected_dense_ts);
                assert_eq!(hits[1].message.ts.as_str(), expected_sparse_newer_ts);
            }
            2 | 3 => {
                assert_eq!(hits.len(), expected_ops_u03_postmortem);
                assert!(hits.iter().all(|h| {
                    h.channel == ChannelId::from("C02")
                        && h.message.user == Some(UserId::from("U03"))
                }));
            }
            4 => {
                assert!(!hits.is_empty());
                let lo = crate::query::Date::parse("2026-01-05")
                    .unwrap()
                    .unix_start()
                    + 86_400;
                let hi = crate::query::Date::parse("2026-01-15")
                    .unwrap()
                    .unix_start();
                assert!(hits.iter().all(|h| {
                    let s = h.message.ts.secs().unwrap();
                    s >= lo && s < hi
                }));
            }
            6 => {
                assert!(!hits.is_empty());
                assert!(hits.iter().all(|h| !h.message.files.is_empty()));
            }
            7 => {
                assert!(!hits.is_empty());
                assert!(hits.iter().all(|h| {
                    h.channel == ChannelId::from("C03") && !h.message.reactions.is_empty()
                }));
            }
            _ => {}
        }
    }

    let search_elapsed = search_start.elapsed();
    latencies_us.sort_unstable();
    let avg_ms = (latencies_us.iter().sum::<u128>() as f64) / (latencies_us.len() as f64) / 1_000.0;
    let p50_ms = latencies_us[latencies_us.len() / 2] as f64 / 1_000.0;
    let p95_ms = latencies_us[(latencies_us.len() * 95) / 100] as f64 / 1_000.0;
    let max_ms = *latencies_us.last().unwrap() as f64 / 1_000.0;
    let stats = store.stats(&workspace).unwrap();

    println!(
        "bench_ingest_and_fts_10k_messages:\n  ingest: {} msgs in {:.2} ms ({:.0} msgs/sec), db_bytes={}\n  search: {} FTS5 queries in {:.2} ms (avg={:.2} ms, p50={:.2} ms, p95={:.2} ms, max={:.2} ms)",
        TOTAL_MESSAGES,
        ingest_elapsed.as_secs_f64() * 1_000.0,
        ingest_msgs_per_sec,
        stats.db_bytes(),
        queries.len(),
        search_elapsed.as_secs_f64() * 1_000.0,
        avg_ms,
        p50_ms,
        p95_ms,
        max_ms,
    );
}

#[test]
fn search_include_deleted_opt_in_returns_tombstones() {
    let mut store = Store::open_in_memory().unwrap();
    let live = msg("1700000100.000100", "U1", "aurora live report");
    let doomed = msg("1700000200.000100", "U1", "aurora deleted secret note");
    store.upsert_message(&ws(), &general(), &live).unwrap();
    store.upsert_message(&ws(), &general(), &doomed).unwrap();
    store
        .record_deletion(
            &ws(),
            &general(),
            &Ts::from("1700000200.000100"),
            None,
            None,
        )
        .unwrap();

    // Default search excludes deleted tombstones.
    let q_default = SearchQuery::parse("aurora");
    let hits = store
        .search_sorted(&ws(), &q_default, SearchSort::Recent, 20)
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].message.ts, Ts::from("1700000100.000100"));
    assert!(!hits[0].deleted);

    // Opt-in search returns both live and locally preserved deleted messages.
    let mut q_all = SearchQuery::parse("aurora");
    q_all.filters.include_deleted = true;
    let hits = store
        .search_sorted(&ws(), &q_all, SearchSort::Recent, 20)
        .unwrap();
    assert_eq!(hits.len(), 2);
    assert!(
        hits.iter()
            .any(|h| h.deleted && h.message.ts == Ts::from("1700000200.000100"))
    );
    assert!(
        hits.iter()
            .any(|h| !h.deleted && h.message.ts == Ts::from("1700000100.000100"))
    );
}

#[test]
fn huddle_transcripts_fts5_multilingual_en_sv_and_sessions() {
    let mut store = Store::open_in_memory().unwrap();
    let ch = ChannelId::from("C0HUDDLE");
    let thread_ts = Ts::from("1700001000.000100");
    let u_alice = UserId::from("U0ALICE");
    let u_bob = UserId::from("U0BOB");

    let seg1 = crate::NewHuddleTranscriptSegment {
        channel: &ch,
        call_id: "R0CALL1",
        thread_root_ts: Some(&thread_ts),
        speaker_id: Some(&u_alice),
        lang: Some("sv"),
        source: "local-whisper",
        text: "Vi måste boka ett möte om utcheckningen imorgon",
        at_ms: 1_700_001_001_000,
    };
    let seg2 = crate::NewHuddleTranscriptSegment {
        channel: &ch,
        call_id: "R0CALL1",
        thread_root_ts: Some(&thread_ts),
        speaker_id: Some(&u_bob),
        lang: Some("en"),
        source: "local-whisper",
        text: "Sounds good, let's verify the checkout deployment before lunch",
        at_ms: 1_700_001_005_000,
    };
    store
        .insert_huddle_transcript_segment(&ws(), &seg1)
        .unwrap();
    store
        .insert_huddle_transcript_segment(&ws(), &seg2)
        .unwrap();

    let call_lines = store.huddle_transcript_for_call(&ws(), "R0CALL1").unwrap();
    assert_eq!(call_lines.len(), 2);
    assert_eq!(call_lines[0].lang.as_deref(), Some("sv"));
    assert_eq!(call_lines[1].lang.as_deref(), Some("en"));

    let by_thread = store
        .huddle_transcript_for_thread(&ws(), &ch, &thread_ts)
        .unwrap();
    assert_eq!(by_thread.len(), 2);
    let counts = store.huddle_transcript_counts_by_thread(&ws()).unwrap();
    assert_eq!(
        counts.get(&(ch.clone(), thread_ts.clone())),
        Some(&("R0CALL1".to_owned(), 2))
    );

    let sessions = store
        .list_huddle_transcript_sessions(&ws(), None, 20)
        .unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].call_id, "R0CALL1");
    assert_eq!(sessions[0].segment_count, 2);
    assert_eq!(sessions[0].languages, vec!["en", "sv"]);

    // Diacritic-insensitive Swedish FTS5 search ("mote" matches "möte").
    let sv_hits = store
        .search_huddle_transcripts(&ws(), "mote", None, None, 20)
        .unwrap();
    assert_eq!(sv_hits.len(), 1);
    assert_eq!(sv_hits[0].speaker_id, Some(u_alice));

    // Language filter narrows bilingual meeting cleanly.
    let en_only = store
        .search_huddle_transcripts(&ws(), "", Some(&ch), Some("en"), 20)
        .unwrap();
    assert_eq!(en_only.len(), 1);
    assert_eq!(en_only[0].speaker_id, Some(u_bob.clone()));

    // Quotes, trailing wildcards, and punctuation-only inputs must not fail FTS5 parsing.
    let quoted_hits = store
        .search_huddle_transcripts(&ws(), r#"deploy* "checkout""#, None, None, 20)
        .unwrap();
    assert_eq!(quoted_hits.len(), 1);
    assert_eq!(quoted_hits[0].speaker_id, Some(u_bob));
    let punct_only = store
        .search_huddle_transcripts(&ws(), r#"""" ***"#, None, None, 20)
        .unwrap();
    assert_eq!(punct_only.len(), 2);

    let rev_before_del = Store::message_revision();
    assert_eq!(store.delete_huddle_transcript(&ws(), "R0CALL1").unwrap(), 2);
    assert!(Store::message_revision() > rev_before_del);
    assert!(
        store
            .search_huddle_transcripts(&ws(), "mote", None, None, 20)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn recent_threads_uses_partial_index_and_filters_correctly() {
    let mut store = Store::open_in_memory().unwrap();
    let plan: Vec<String> = {
        let explain_sql = format!("EXPLAIN QUERY PLAN {RECENT_THREADS_WORKSPACE_SQL}");
        let mut stmt = store.conn.prepare(&explain_sql).unwrap();
        let rows: Vec<String> = stmt
            .query_map(params![ws().as_str(), 10i64], |r| r.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        drop(stmt);
        rows
    };
    assert!(
        plan.iter()
            .any(|detail| detail.contains("USING INDEX message_current_threads")),
        "expected recent_threads to use partial index message_current_threads, got plan: {plan:?}"
    );

    let mut root1 = msg("1700000001.000100", "U1", "thread one");
    root1.reply_count = Some(2);
    let mut root2 = msg("1700000003.000100", "U2", "thread two");
    root2.reply_count = Some(5);
    let mut zero_replies = msg("1700000004.000100", "U1", "zero replies");
    zero_replies.reply_count = Some(0);
    let plain = msg("1700000005.000100", "U1", "plain message");
    let mut deleted_root = msg("1700000006.000100", "U1", "deleted thread");
    deleted_root.reply_count = Some(4);

    store
        .upsert_messages(&ws(), &general(), &[root1, zero_replies, plain])
        .unwrap();
    store
        .upsert_message(&ws(), &ChannelId::from("C2"), &root2)
        .unwrap();
    store
        .upsert_message(&ws(), &general(), &deleted_root)
        .unwrap();
    store
        .record_deletion(
            &ws(),
            &general(),
            &deleted_root.ts,
            None,
            Some(&deleted_root),
        )
        .unwrap();

    let all = store.recent_threads(&ws(), None, 10).unwrap();
    let all_texts: Vec<&str> = all.iter().map(|m| m.message.text.as_str()).collect();
    assert_eq!(all_texts, ["thread two", "thread one"]);

    let c1_only = store.recent_threads(&ws(), Some(&general()), 10).unwrap();
    let c1_texts: Vec<&str> = c1_only.iter().map(|m| m.message.text.as_str()).collect();
    assert_eq!(c1_texts, ["thread one"]);
}

#[test]
fn empty_upsert_does_not_bump_revision_while_transcript_insert_does() {
    let mut store = Store::open_in_memory().unwrap();
    let rev = Store::message_revision();
    store.upsert_messages(&ws(), &general(), &[]).unwrap();
    assert_eq!(Store::message_revision(), rev);

    let seg = crate::NewHuddleTranscriptSegment {
        channel: &general(),
        call_id: "R0REV",
        thread_root_ts: None,
        speaker_id: None,
        lang: Some("en"),
        source: "chime",
        text: "   ",
        at_ms: 1_700_000_000_000,
    };
    assert_eq!(
        store.insert_huddle_transcript_segment(&ws(), &seg).unwrap(),
        0
    );
    assert_eq!(Store::message_revision(), rev);

    let seg_non_empty = crate::NewHuddleTranscriptSegment {
        text: "hello huddle",
        ..seg
    };
    assert!(
        store
            .insert_huddle_transcript_segment(&ws(), &seg_non_empty)
            .unwrap()
            > 0
    );
    assert!(Store::message_revision() > rev);
}
