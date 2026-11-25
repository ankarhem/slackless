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
    assert_eq!(ts_key(&Ts::from("")), 0);
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

    let mut reader = Store::open_read_only(&path).unwrap();
    assert_eq!(
        reader.local(&ws(), "theme").unwrap().as_deref(),
        Some("dark")
    );
    assert!(reader.set_local(&ws(), "theme", "light").is_err());
    assert!(
        reader
            .upsert_message(&ws(), &general(), &msg("1700000001.000100", "U1", "nope"))
            .is_err()
    );
    assert!(reader.enqueue_offline(&ws(), "{}").is_err());

    drop(reader);

    // Bump user_version past SCHEMA_VERSION and verify Store::open returns FutureSchema.
    let future = schema::SCHEMA_VERSION + 2;
    writer
        .conn
        .pragma_update(None, "user_version", future)
        .unwrap();
    drop(writer);

    let err = match Store::open(&path) {
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

    let _ = std::fs::remove_dir_all(&dir);
}
