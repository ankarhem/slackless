use super::*;
use crate::query::SearchFilters;
use crate::types::{Edited, File, FileId, Reaction, SectionKind, UserId};

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
              'reaction','team','channel_section','custom_emoji','prefs')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tables, 10);
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
            is_bot: i % 10 == 0,
            deleted: i % 25 == 0,
            team_id: Some(if i % 50 == 0 {
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
