use super::orgs::{created_on, filter_choices, team_icon_url};
use super::*;
use slackless_core::types::{Team, TeamId, User};

fn team(id: &str, name: &str) -> Team {
    Team {
        id: TeamId::from(id),
        name: name.to_owned(),
        domain: name.to_lowercase(),
        ..Default::default()
    }
}

fn user(id: &str, team: Option<&str>) -> User {
    User {
        id: UserId::from(id),
        team_id: team.map(TeamId::from),
        ..Default::default()
    }
}

#[test]
fn orgs_count_members_per_team_with_own_first() {
    let own = TeamId::from("T0ACME");
    let members = ["U1", "U2", "UE1", "UE2", "UE3", "UU"]
        .map(UserId::from)
        .to_vec();
    let users = vec![
        user("U1", Some("T0ACME")),
        user("U2", Some("T0ACME")),
        user("UE1", Some("T0ZETA")),
        user("UE2", Some("T0KLARNA")),
        user("UE3", Some("T0ZETA")),
        // UU has no profile: an own-workspace person waiting on `users.info`.
    ];
    let teams = vec![
        team("T0KLARNA", "Klarna"),
        team("T0ZETA", "Zeta"),
        team("T0ACME", "Acme"),
    ];

    let lines = orgs(&own, &members, &users, &teams);
    let summary: Vec<(&str, u32)> = lines
        .iter()
        .map(|line| (line.team.name.as_str(), line.members))
        .collect();
    assert_eq!(
        summary,
        [("Acme", 3), ("Klarna", 1), ("Zeta", 2)],
        "own org first (with the unprofiled member), connected by name"
    );
}

#[test]
fn orgs_skip_bots_and_members_of_unknown_teams() {
    let own = TeamId::from("T0ACME");
    let members = ["U1", "UBOT", "UE1"].map(UserId::from).to_vec();
    let users = vec![
        user("U1", Some("T0ACME")),
        User {
            id: UserId::from("UBOT"),
            is_bot: true,
            ..Default::default()
        },
        user("UE1", Some("T0GHOST")),
    ];
    let teams = vec![team("T0ACME", "Acme")];

    let lines = orgs(&own, &members, &users, &teams);
    assert_eq!(lines.len(), 1, "the bot never lists, T0GHOST has no record");
    assert_eq!((lines[0].team.name.as_str(), lines[0].members), ("Acme", 1));
}

#[test]
fn listed_members_keep_unknowns_and_drop_bots() {
    let members = ["U1", "UBOT", "UGONE", "UU", "USLACKBOT"]
        .map(UserId::from)
        .to_vec();
    let users = vec![
        user("U1", Some("T1")),
        User {
            id: UserId::from("UBOT"),
            is_bot: true,
            ..Default::default()
        },
        User {
            id: UserId::from("UGONE"),
            deleted: true,
            ..Default::default()
        },
    ];
    let profiles = profiles_by_id(&users);
    let listed: Vec<&UserId> = members
        .iter()
        .filter(|member| is_listed(member, &profiles))
        .collect();
    assert_eq!(listed, [&UserId::from("U1"), &UserId::from("UU")]);
}

#[test]
fn created_on_formats_slacks_date() {
    assert_eq!(created_on(1_593_043_200), "June 25, 2020");
    assert_eq!(created_on(0), "January 1, 1970");
    assert_eq!(created_on(1_633_305_600), "October 4, 2021");
}

#[test]
fn team_icon_url_picks_the_smallest_size() {
    let mut record = team("T1", "Acme");
    record.icon = Some(crate::blockkit::serde_json::json!({
        "image_default": true,
        "image_34": "https://img.example/34.png",
        "image_132": "https://img.example/132.png"
    }));
    assert_eq!(
        team_icon_url(&record).as_deref(),
        Some("https://img.example/34.png")
    );
    let bare = team("T2", "Bare");
    assert_eq!(team_icon_url(&bare), None, "no icon object, no url");
}

#[test]
fn filter_choices_follow_slacks_order_and_counts() {
    let mut fixtures = crate::fixtures::load();
    fixtures.workspaces = vec![team("T0ACME", "Acme Corp")];
    fixtures.teams = vec![team("T0KLARNA", "Klarna"), team("T0ZETA", "Zeta")];
    let row = |team: Option<&str>, manager: bool, guest: bool| MemberRow {
        id: UserId::from("U1"),
        name: "A Person".to_owned(),
        key: "a person".to_owned(),
        team: team.map(TeamId::from),
        handle: None,
        job: None,
        emoji: None,
        manager,
        guest,
    };
    let rows = vec![
        row(None, true, false),
        row(None, false, false),
        row(None, false, true),
        row(Some("T0ZETA"), false, false),
        row(Some("T0KLARNA"), false, false),
        row(Some("T0ZETA"), false, false),
        row(Some("T0KLARNA"), false, true),
    ];
    let choices = filter_choices(&fixtures, &rows);
    let got: Vec<(&str, usize)> = choices
        .iter()
        .map(|choice| (choice.label.as_ref(), choice.count))
        .collect();
    assert_eq!(
        got,
        [
            ("All", 7),
            ("External organizations", 3),
            ("Channel Managers", 1),
            ("Members", 1),
            ("Guests", 2),
            ("Acme Corp", 3),
            ("Zeta", 2),
            ("Klarna", 2),
        ]
    );
    // An internal channel: neither the external choice nor the "People from" group.
    let internal = filter_choices(&fixtures, &rows[..3]);
    let labels: Vec<&str> = internal
        .iter()
        .map(|choice| choice.label.as_ref())
        .collect();
    assert_eq!(labels, ["All", "Channel Managers", "Members", "Guests"]);
}
