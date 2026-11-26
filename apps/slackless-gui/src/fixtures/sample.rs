//! Bundled sample workspace ("Acme Corp" and "Side Project") used in offline / demo / test mode.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock, RwLock};

use serde_json::{Value as Json, json};
use slackless_core::NotifyLevel;
use slackless_core::types::{
    Attachment, Block, ChannelId, Conversation, ConversationProperties, Edited, File, FileId,
    Message, MessageSubtype, Presence, Profile, Reaction, RichTextBlock, RichTextContainer,
    RichTextElement, Team, TextStyle, TopicPurpose, Ts, User, UserId, Usergroup, UsergroupId,
    message::AttachmentField,
    rich_text::{
        EmojiElement, LinkElement, ListStyle, RichTextList, RichTextPreformatted, RichTextSection,
        TextElement, UserElement,
    },
};

use super::model::{BotProfileMap, EmojiImage, Fixtures, ProfileExtras, WorkspaceData};
use crate::thumbnail;

/// Seconds between sample days, so scrollbacks span several date dividers.
const DAY: u64 = 86_400;

/// Unix seconds the sample scrollback starts at, anchored to render time: the three sample
/// days land on "2 days ago / yesterday / today", like a live workspace's history.
fn base_ts() -> u64 {
    let now = slackless_core::types::time::now_epoch_secs().max(0) as u64;
    (now / DAY).saturating_sub(2) * DAY + 8 * 3_600
}

/// Each conversation's newest history message, the fixture-mode [`Fixtures::latest`].
fn latest_from_histories(
    messages: &HashMap<ChannelId, Vec<Message>>,
) -> HashMap<ChannelId, Message> {
    messages
        .iter()
        .filter_map(|(channel, history)| {
            history
                .last()
                .map(|latest| (channel.clone(), latest.clone()))
        })
        .collect()
}

pub fn load() -> Fixtures {
    let workspaces = vec![
        Team {
            id: "T0ACME".into(),
            name: "Acme Corp".into(),
            domain: "acme".into(),
            ..Default::default()
        },
        Team {
            id: "T0SIDE".into(),
            name: "Side Project".into(),
            domain: "side".into(),
            ..Default::default()
        },
    ];

    let mut users = vec![
        user("U0ME", "sam", "Sam Lindqvist"),
        user("U0ALICE", "alice", "Alice Andersson"),
        user("U0BOB", "bob", "Bob Berg"),
        user("U0CARA", "cara", "Cara Chen"),
        user("U0DAN", "dan", "Dan Duarte"),
    ];
    // (title, IANA zone, zone label, UTC offset hours, phone, status emoji, status text)
    let details = [
        (
            "Staff Engineer",
            "Europe/Stockholm",
            "Central European Time",
            1,
            "+46 70 123 45 67",
            "",
            "",
        ),
        (
            "Engineering Manager",
            "Europe/Stockholm",
            "Central European Time",
            1,
            "+46 70 555 01 02",
            ":calendar:",
            "In meetings until 15:00",
        ),
        (
            "Site Reliability Engineer",
            "Europe/London",
            "Greenwich Mean Time",
            0,
            "+44 20 7946 0018",
            ":palm_tree:",
            "Vacationing",
        ),
        (
            "Product Designer",
            "America/New_York",
            "Eastern Standard Time",
            -5,
            "+1 212 555 0147",
            "",
            "",
        ),
        (
            "Backend Engineer",
            "America/Los_Angeles",
            "Pacific Standard Time",
            -8,
            "+1 415 555 0199",
            ":house_with_garden:",
            "Working remotely",
        ),
    ];
    for (user, (title, tz, tz_label, offset, phone, emoji, status)) in users.iter_mut().zip(details)
    {
        user.tz = Some(tz.into());
        user.tz_label = Some(tz_label.into());
        user.tz_offset = Some(offset * 3_600);
        user.profile.title = Some(title.into());
        user.profile.phone = Some(phone.into());
        user.profile.email = Some(format!("{}@acme.example", user.name));
        if !status.is_empty() {
            user.profile.status_emoji = Some(emoji.into());
            user.profile.status_text = Some(status.into());
            user.profile.status_expiration = Some(0);
        }
    }
    let profile_extras = [
        (
            "U0ME",
            "he/him",
            "Platform",
            "Alice Andersson",
            "2021-03-01",
        ),
        (
            "U0ALICE",
            "she/her",
            "Platform",
            "Dana Okafor",
            "2019-08-12",
        ),
        (
            "U0BOB",
            "he/him",
            "Infrastructure",
            "Alice Andersson",
            "2022-01-10",
        ),
        ("U0CARA", "she/her", "Design", "Dana Okafor", "2020-05-04"),
        (
            "U0DAN",
            "they/them",
            "Platform",
            "Alice Andersson",
            "2023-09-18",
        ),
    ]
    .into_iter()
    .map(|(id, pronouns, team, manager, start)| {
        let extras = ProfileExtras {
            pronouns: Some(pronouns.into()),
            fields: vec![
                ("Team".into(), team.into()),
                ("Reports to".into(), manager.into()),
                ("Start date".into(), start.into()),
            ],
        };
        (UserId::from(id), extras)
    })
    .collect();

    // The two members of the connected organization in the Slack Connect channel.
    for (id, name, real) in [
        ("U0KLA1", "elin", "Elin Ekman"),
        ("U0KLA2", "johannes", "Johannes Jonsson"),
    ] {
        let mut external = user(id, name, real);
        external.team_id = Some("T0KLARNA".into());
        users.push(external);
    }

    let mut channels = vec![
        channel(
            "C0GENERAL",
            "general",
            "Company-wide announcements",
            false,
            128,
        ),
        channel("C0RANDOM", "random", "Non-work banter", false, 96),
        channel(
            "C0ENG",
            "engineering",
            "Ship it. Deploys at 14:00.",
            false,
            42,
        ),
        channel("C0DESIGN", "design", "Private design reviews", true, 7),
    ];
    channels[0].is_general = true;
    // The creators and creation dates of the sample channels (Slack's details dialog).
    for (ix, creator, created) in [
        (0, "U0ALICE", 1_565_568_000), // 2019-08-12
        (1, "U0BOB", 1_565_654_400),   // 2019-08-13
        (2, "U0ALICE", 1_593_043_200), // 2020-06-25
        (3, "U0CARA", 1_633_305_600),  // 2021-10-04
    ] {
        channels[ix].creator = Some(creator.into());
        channels[ix].created = Some(created);
    }

    // A Slack Connect channel shared with Klarna: 3 Acme members + 2 from the connected
    // organization, hosted by them, like the reference capture's partner channel.
    let mut partner = channel(
        "C0PARTNER",
        "partner-support-klarna",
        "Escalations with Klarna's support desk",
        true,
        5,
    );
    partner.is_shared = true;
    partner.is_ext_shared = true;
    partner.purpose = Some(TopicPurpose {
        value: "Escalation lane between Acme support and Klarna".into(),
        ..Default::default()
    });
    partner.creator = Some("U0KLA1".into());
    partner.created = Some(1_593_043_200); // 2020-06-25
    partner.context_team_id = Some("T0KLARNA".into());
    partner.members = vec![
        "U0ME".into(),
        "U0ALICE".into(),
        "U0BOB".into(),
        "U0KLA1".into(),
        "U0KLA2".into(),
    ];
    let mut partner_extra = serde_json::Map::new();
    partner_extra.insert(
        "conversation_host_id".to_owned(),
        serde_json::Value::String("T0KLARNA".to_owned()),
    );
    partner.extra = partner_extra;
    channels.push(partner);

    // A dormant channel ("No messages in at least 30 days"): Slack's "Active only" default
    // hides it from the list, so it only surfaces through the section popover and menus.
    let mut archive = channel(
        "C0ARCHIVE",
        "archive-talk",
        "Planning threads from the old roadmap",
        false,
        3,
    );
    let mut archive_properties = ConversationProperties::default();
    archive_properties
        .extra
        .insert("is_dormant".to_owned(), serde_json::Value::Bool(true));
    archive.properties = Some(archive_properties);
    channels.push(archive);

    let directory = [
        (
            "C0OPS",
            "ops-alerts",
            "PagerDuty pages and incident chatter",
            23,
        ),
        (
            "C0SUPPORT",
            "support",
            "Customer escalations, weekdays 8-17",
            31,
        ),
        (
            "C0MARKETING",
            "marketing",
            "Campaigns, launches and brand",
            18,
        ),
        (
            "C0BOOKS",
            "book-club",
            "This month: The Mythical Man-Month",
            11,
        ),
    ]
    .into_iter()
    .map(|(id, name, topic, members)| {
        let mut conversation = channel(id, name, topic, false, members);
        conversation.is_member = false;
        conversation
    })
    .collect();

    let presence = HashMap::from([
        ("U0ME".into(), Presence::Active),
        ("U0ALICE".into(), Presence::Active),
        ("U0BOB".into(), Presence::Away),
        ("U0CARA".into(), Presence::Active),
        ("U0DAN".into(), Presence::Away),
    ]);

    let dms = vec![
        Conversation {
            id: "D0ALICE".into(),
            is_im: true,
            user: Some("U0ALICE".into()),
            // Read up to the first message: the reply renders the "New" divider, like the
            // reference screenshot's unread DM.
            last_read: Some(ts(2 * DAY + 9_000)),
            ..Default::default()
        },
        Conversation {
            id: "D0BOB".into(),
            is_im: true,
            user: Some("U0BOB".into()),
            ..Default::default()
        },
        Conversation {
            id: "G0TRIO".into(),
            is_mpim: true,
            members: vec!["U0ME".into(), "U0CARA".into(), "U0DAN".into()],
            ..Default::default()
        },
    ];

    // #general: three days of company chatter, plus the reaction-heavy messages.
    let mut general = vec![
        msg(
            "C0GENERAL",
            900,
            "U0ALICE",
            "Good morning everyone! Reminder: all-hands at 15:00 today.",
        ),
        msg(
            "C0GENERAL",
            960,
            "U0BOB",
            "Thanks Alice. Will there be a recording?",
        ),
        msg(
            "C0GENERAL",
            1_020,
            "U0ALICE",
            "Yes, it will be posted in #general afterwards.",
        ),
        msg(
            "C0GENERAL",
            DAY + 600,
            "U0CARA",
            "The new office coffee machine is finally installed.",
        ),
        msg_rich(
            "C0GENERAL",
            DAY + 660,
            "U0DAN",
            "Best news this quarter. :tada:",
            vec![section(vec![
                plain("Best news "),
                styled(
                    "this quarter",
                    TextStyle {
                        bold: true,
                        ..Default::default()
                    },
                ),
                plain(". "),
                emoji("tada", "1f389"),
            ])],
        ),
        msg(
            "C0GENERAL",
            DAY + 900,
            "U0ME",
            "Welcome to our two new hires starting this week!",
        ),
    ];
    general[2].edited = Some(Edited {
        user: Some("U0ALICE".into()),
        ts: ts(1_025),
    });
    general[3].reactions = vec![
        reaction("coffee", &["U0BOB", "U0DAN", "U0ME"]),
        reaction("tada", &["U0ALICE"]),
    ];
    general[5].reactions = vec![reaction("wave", &["U0ALICE", "U0BOB", "U0CARA", "U0DAN"])];

    // #engineering: a rich deploy report and the latency thread used by the thread panel.
    let mut eng = vec![
        msg_rich(
            "C0ENG",
            3_600,
            "U0BOB",
            "Deploy of api v2.4 is green on staging: https://grafana.acme.example/api-v2",
            vec![
                section(vec![
                    plain("Deploy of "),
                    styled(
                        "api v2.4",
                        TextStyle {
                            bold: true,
                            ..Default::default()
                        },
                    ),
                    plain(" is green on staging: "),
                    link("https://grafana.acme.example/api-v2", "dashboard"),
                ]),
                code_block("bash", "cargo deploy --service api --env staging"),
            ],
        ),
        msg_rich(
            "C0ENG",
            3_700,
            "U0CARA",
            "I'm seeing elevated p99 latency on /search since the deploy. Thread for details.",
            vec![section(vec![
                plain("I'm seeing "),
                styled(
                    "elevated",
                    TextStyle {
                        bold: true,
                        ..Default::default()
                    },
                ),
                plain(" "),
                styled(
                    "p99",
                    TextStyle {
                        code: true,
                        ..Default::default()
                    },
                ),
                plain(" latency on "),
                styled(
                    "/search",
                    TextStyle {
                        code: true,
                        ..Default::default()
                    },
                ),
                plain(" since the deploy. Thread for details."),
            ])],
        ),
        msg_rich(
            "C0ENG",
            DAY + 1_500,
            "U0ALICE",
            "Reminder: code freeze starts Friday.",
            vec![section(vec![
                plain("Reminder: "),
                styled(
                    "code freeze starts Friday",
                    TextStyle {
                        italic: true,
                        ..Default::default()
                    },
                ),
                plain("."),
            ])],
        ),
        msg(
            "C0ENG",
            DAY + 1_560,
            "U0ME",
            "Merged the reconnect backoff fix, should cut the websocket churn.",
        ),
    ];
    let eng_thread_root = eng[1].ts.clone();
    eng[0].reactions = vec![reaction("rocket", &["U0ALICE", "U0CARA", "U0DAN"])];
    eng[1].thread_ts = Some(eng_thread_root.clone());
    eng[1].reply_count = Some(3);
    eng[1].reply_users_count = Some(2);
    eng[1].reply_users = vec!["U0BOB".into(), "U0ME".into()];
    eng[1].latest_reply = Some(ts(3_980));
    eng[1].reactions = vec![reaction("eyes", &["U0BOB", "U0ME"])];
    eng[3].reactions = vec![reaction("rocket", &["U0BOB", "U0CARA"])];
    eng[0]
        .reactions
        .push(reaction("shipit", &["U0BOB", "U0ME"]));
    eng.push(release_message(DAY + 1_620));
    // A blank line inside one rich-text section, the way Slack's API packs it into a
    // `\n\n`-leading text element (2026-11-21, live norce-partner-announcements data).
    // Last in the channel so the tail-anchored transcript shows it without jumping.
    eng.push(msg_rich(
        "C0ENG",
        DAY + 1_640,
        "U0DAN",
        "Rollout notes:\n\nStage one is done, stage two next.",
        vec![section(vec![
            plain("Rollout notes:"),
            plain("\n\nStage one is done, "),
            styled(
                "stage two",
                TextStyle {
                    bold: true,
                    ..Default::default()
                },
            ),
            plain(" next."),
        ])],
    ));
    let eng_replies = vec![
        msg(
            "C0ENG",
            3_760,
            "U0BOB",
            "Looks like the new FTS query plan. Checking.",
        ),
        msg(
            "C0ENG",
            3_820,
            "U0ME",
            "Could be the missing index on the join, I can pair on it.",
        ),
        msg(
            "C0ENG",
            3_980,
            "U0BOB",
            "Index added, p99 is back to normal.",
        ),
    ]
    .into_iter()
    .map(|mut m| {
        m.thread_ts = Some(eng_thread_root.clone());
        m.parent_user_id = Some("U0CARA".into());
        m
    })
    .collect();

    // #random: a user mention inline and a reaction on the reply.
    let mut random = vec![
        msg_rich(
            "C0RANDOM",
            2 * DAY + 300,
            "U0DAN",
            "<@U0ME> anyone up for lunch at the ramen place?",
            vec![section(vec![
                mention("U0ME"),
                plain(" anyone up for lunch at the ramen place?"),
            ])],
        ),
        msg("C0RANDOM", 2 * DAY + 360, "U0CARA", "In! 12:15?"),
        msg("C0RANDOM", 2 * DAY + 420, "U0ME", "Count me in."),
    ];
    random[2].reactions = vec![reaction("thumbsup", &["U0CARA", "U0DAN"])];

    // #design: external link inside a rich message.
    let mut design = vec![
        msg_rich(
            "C0DESIGN",
            DAY + 7_200,
            "U0CARA",
            "New sidebar mockups are up for review: https://figma.com/file/acme-sidebar-v3",
            vec![section(vec![
                plain("New sidebar mockups are up for review: "),
                link(
                    "https://figma.com/file/acme-sidebar-v3",
                    "Figma: sidebar v3",
                ),
            ])],
        ),
        msg(
            "C0DESIGN",
            DAY + 7_260,
            "U0ALICE",
            "Love the denser channel list.",
        ),
        msg(
            "C0DESIGN",
            2 * DAY + 8_000,
            "U0ME",
            "Left a few comments in Figma.",
        ),
    ];
    design[0].files = vec![image_file("F0SIDEBAR", "sidebar-v3.png", 184_320, 320, 200)];
    design[1].files = vec![doc_file(
        "F0SPECS",
        "sidebar-specs.xlsx",
        "xlsx",
        "Excel Spreadsheet",
        52_736,
    )];
    design[2].files = vec![File {
        id: FileId::from("F0TOKENS"),
        name: Some("Chalk_Custom.terminal".into()),
        title: Some("Chalk_Custom.terminal".into()),
        filetype: Some("xml".into()),
        pretty_type: Some("Plain Text".into()),
        mode: Some(slackless_core::types::FileMode::Snippet),
        preview: Some(
            concat!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
                "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
                "<plist version=\"1.0\">\n",
                "<dict>\n",
                "    <key>ANSIBlackColor</key>\n",
                "    <data>YnBsaXN0MDDUAQIDBAUGBwpYJHZlcnNpb25ZJGFyY2hpdmVy</data>\n",
                "    <key>ANSIRedColor</key>\n",
                "    <data>YnBsaXN0MDDUAQIDBAUGBwpYJHZlcnNpb25ZJGFyY2hpdmVy</data>\n",
                "    <key>ANSIGreenColor</key>\n",
                "    <data>YnBsaXN0MDDUAQIDBAUGBwpYJHZlcnNpb25ZJGFyY2hpdmVy</data>\n",
                "</dict>\n",
                "</plist>\n",
            )
            .into(),
        ),
        ..Default::default()
    }];

    let mut dm_alice = vec![
        msg(
            "D0ALICE",
            2 * DAY + 9_000,
            "U0ALICE",
            "Hey, do you have a minute to look at the Q4 plan?",
        ),
        msg("D0ALICE", 2 * DAY + 9_060, "U0ME", "Sure, send it over."),
        msg(
            "D0ALICE",
            2 * DAY + 9_120,
            "U0ALICE",
            "Here it is, plus the dashboard mockup we discussed.",
        ),
    ];
    dm_alice[2].files = vec![
        doc_file("F0Q4PLAN", "Q4-plan.pdf", "pdf", "PDF", 2_457_600),
        image_file("F0DASH", "dashboard-mockup.png", 230_400, 360, 225),
    ];

    let thumbnails = HashMap::from([
        ("F0DASH".into(), thumbnail::mockup(360, 225, 0x1D_9B_D1)),
        ("F0SIDEBAR".into(), thumbnail::mockup(320, 200, 0x7C_5C_FF)),
    ]);
    let custom_emoji = [
        ("shipit", 0x2E_B6_7D, 0xEC_B2_2E),
        ("partyparrot", 0xE0_1E_5A, 0x36_C5_F0),
        ("lgtm", 0x36_C5_F0, 0xFF_FF_FF),
        ("this-is-fine", 0xEC_B2_2E, 0xE0_1E_5A),
        ("acme", 0x7C_5C_FF, 0x1D_9B_D1),
    ]
    .into_iter()
    .map(|(name, accent, inner)| {
        let image = thumbnail::emblem(64, accent, inner);
        (name.to_owned(), EmojiImage::Sample(image))
    })
    .collect();
    let block_images = HashMap::from([
        (
            LATENCY_CHART.to_owned(),
            thumbnail::mockup(400, 172, 0x2B_AC_76),
        ),
        (
            CI_ICON.to_owned(),
            thumbnail::emblem(32, 0x2E_B6_7D, 0xFF_FF_FF),
        ),
    ]);
    let dm_bob = vec![msg_rich(
        "D0BOB",
        2 * DAY + 9_200,
        "U0BOB",
        "PR is ready for review: https://github.com/acme/api/pull/482",
        vec![section(vec![
            plain("PR is ready for review when you get a chance: "),
            link("https://github.com/acme/api/pull/482", "acme/api#482"),
        ])],
    )];

    // Trio MPIM: a bullet list inside a rich message.
    let trio = vec![
        msg_rich(
            "G0TRIO",
            DAY + 10_000,
            "U0DAN",
            "Offsite venue options: A fjord cabin, B city hotel, C lakeside lodge.",
            vec![
                section(vec![plain(
                    "Offsite venue options are in the doc. Quick summary:",
                )]),
                bullet_list(&[
                    "A: fjord cabin, sleeps 12",
                    "B: city hotel with rooftop view",
                    "C: lakeside lodge",
                ]),
            ],
        ),
        msg(
            "G0TRIO",
            DAY + 10_060,
            "U0CARA",
            "Option B has the better view.",
        ),
    ];

    // The Slack Connect channel's history: a question from the connected org, answered in
    // kind, so external authors render with their profiles.
    let partner = vec![
        msg(
            "C0PARTNER",
            500,
            "U0KLA1",
            "Hej! Dispute #4812 needs Acme's ledger entries from Tuesday.",
        ),
        msg(
            "C0PARTNER",
            DAY + 560,
            "U0ALICE",
            "Pulling them now — expect an export in a few minutes.",
        ),
        msg(
            "C0PARTNER",
            DAY + 620,
            "U0KLA2",
            "Tack! We'll keep the dispute open until then.",
        ),
    ];

    let messages = HashMap::from([
        ("C0PARTNER".into(), partner),
        ("C0GENERAL".into(), general),
        ("C0RANDOM".into(), random),
        ("C0ENG".into(), eng),
        ("C0DESIGN".into(), design),
        ("D0ALICE".into(), dm_alice),
        ("D0BOB".into(), dm_bob),
        ("G0TRIO".into(), trio),
    ]);
    let threads = HashMap::from([(("C0ENG".into(), eng_thread_root), eng_replies)]);
    let latest = latest_from_histories(&messages);

    // The Acme record reappears as the own team of [`Fixtures::teams`], beside Klarna's.
    let teams = vec![
        workspaces[0].clone(),
        Team {
            id: "T0KLARNA".into(),
            name: "Klarna".into(),
            domain: "klarna".into(),
            ..Default::default()
        },
    ];

    let usergroups = vec![
        Usergroup {
            id: UsergroupId::from("S0ENG"),
            name: "Engineering".into(),
            handle: "engineering".into(),
            description: Some("Product and platform engineers".into()),
            user_count: Some(6),
            ..Default::default()
        },
        Usergroup {
            id: UsergroupId::from("S0DES"),
            name: "Design".into(),
            handle: "design".into(),
            description: Some("Product designers".into()),
            user_count: Some(3),
            ..Default::default()
        },
    ];

    Fixtures::from_parts(
        WorkspaceData {
            workspaces,
            teams,
            self_user: "U0ME".into(),
            users,
            usergroups,
            app_commands: Vec::new(),
            channels,
            dms,
            messages,
            latest,
            threads,
            directory,
            archived: Vec::new(),
            presence: Arc::new(RwLock::new(presence)),
            bots: BotProfileMap::default(),
            notify_levels: HashMap::from([(ChannelId::from("C0DESIGN"), NotifyLevel::Muted)]),
            quick_reactions: crate::actions::DEFAULT_QUICK_REACTIONS
                .map(String::from)
                .to_vec(),
            frequent_emoji: Vec::new(),
            skin_tone: 1,
            thumbnails,
            file_transcripts: HashMap::new(),
            file_transcript_cues: HashMap::new(),
            profile_extras,
            custom_emoji,
            block_images,
            sample: true,
            sections: Vec::new(),
            real_names: false,
            real_names_in_mentions: false,
            collapsed_sections: HashSet::new(),
            palette: None,
            status_presets: Vec::new(),
            user_index: OnceLock::new(),
            conv_index: OnceLock::new(),
            team_index: OnceLock::new(),
        },
        HashMap::new(),
    )
}

/// The fixtures of workspace `index` in [`Fixtures::workspaces`]: Acme Corp is [`load`], the
/// Side Project workspace swaps in its own conversations and history. Same signed-in user,
/// people and emoji in both, as one Slack account signed in to two workspaces.
pub fn workspace(index: usize) -> Fixtures {
    let mut fixtures = load();
    if index == 0 {
        return fixtures;
    }
    let mut channels = vec![
        channel("C1GENERAL", "general", "Side Project HQ", false, 4),
        channel("C1IDEAS", "ideas", "Half-baked is fine here", false, 3),
        channel(
            "C1RELEASES",
            "releases",
            "Changelog and launch plans",
            false,
            4,
        ),
    ];
    channels[0].is_general = true;
    let mut bugs = channel("C1BUGS", "bug-reports", "Beta tester reports", false, 2);
    bugs.is_member = false;
    fixtures.channels = channels;
    fixtures.directory = vec![bugs];
    fixtures.dms = vec![
        Conversation {
            id: "D1CARA".into(),
            is_im: true,
            user: Some("U0CARA".into()),
            ..Default::default()
        },
        Conversation {
            id: "D1DAN".into(),
            is_im: true,
            user: Some("U0DAN".into()),
            ..Default::default()
        },
    ];
    fixtures.messages = HashMap::from([
        (
            "C1GENERAL".into(),
            vec![
                msg(
                    "C1GENERAL",
                    1_200,
                    "U0DAN",
                    "Weekend sync: landing page is live on the staging domain.",
                ),
                msg(
                    "C1GENERAL",
                    DAY + 2_000,
                    "U0CARA",
                    "New logo options are in the Figma file.",
                ),
                msg(
                    "C1GENERAL",
                    2 * DAY + 3_000,
                    "U0ME",
                    "Nice! I'll review tonight.",
                ),
            ],
        ),
        (
            "C1IDEAS".into(),
            vec![
                msg(
                    "C1IDEAS",
                    DAY + 4_000,
                    "U0CARA",
                    "What if onboarding was a single page?",
                ),
                msg(
                    "C1IDEAS",
                    2 * DAY + 5_000,
                    "U0DAN",
                    "Dark mode by default, light as opt-in.",
                ),
            ],
        ),
        (
            "C1RELEASES".into(),
            vec![msg(
                "C1RELEASES",
                2 * DAY + 6_000,
                "U0DAN",
                "v0.3.0 tagged: offline drafts and faster sync.",
            )],
        ),
        (
            "D1CARA".into(),
            vec![msg(
                "D1CARA",
                2 * DAY + 7_000,
                "U0CARA",
                "Can you look at the pricing copy before Friday?",
            )],
        ),
        (
            "D1DAN".into(),
            vec![msg(
                "D1DAN",
                DAY + 8_000,
                "U0DAN",
                "Pushed the auth fix, CI is green.",
            )],
        ),
    ]);
    fixtures.threads = HashMap::new();
    fixtures.latest = latest_from_histories(&fixtures.messages);
    fixtures
}

/// Fixture image URLs (no network: [`Fixtures::block_images`] holds their decoded pixels).
const LATENCY_CHART: &str = "https://fixtures.slackless/latency.png";
const CI_ICON: &str = "https://fixtures.slackless/ci.png";

/// A deploy-bot message exercising every Block Kit block a client must display (header,
/// section with fields and accessories, divider, actions, context, image, inputs, rich_text)
/// plus two legacy attachments with color bars and fields.
fn release_message(offset: u64) -> Message {
    let label = |text: &str| json!({ "type": "plain_text", "text": text, "emoji": true });
    let option = |text: &str, value: &str| json!({ "text": label(text), "value": value });
    let blocks: Vec<Json> = vec![
        json!({ "type": "header", "text": label(":rocket: Release v2.5.0 is ready for production") }),
        json!({
            "type": "section",
            "block_id": "summary",
            "text": { "type": "mrkdwn", "text": "*api v2.5.0* passed every check on staging. <@U0BOB> requested a production rollout, see the <https://github.com/acme/api/compare/v2.4.3...v2.5.0|diff>." },
            "accessory": { "type": "button", "action_id": "view_diff", "text": label("View diff"), "url": "https://github.com/acme/api/compare/v2.4.3...v2.5.0" }
        }),
        json!({
            "type": "section",
            "block_id": "facts",
            "fields": [
                { "type": "mrkdwn", "text": "*Service*\napi" },
                { "type": "mrkdwn", "text": "*Environment*\nproduction" },
                { "type": "mrkdwn", "text": "*Commits*\n14 since v2.4.3" },
                { "type": "mrkdwn", "text": "*Requested by*\n<@U0BOB>" }
            ]
        }),
        json!({ "type": "divider" }),
        json!({
            "type": "section",
            "block_id": "strategy",
            "text": { "type": "mrkdwn", "text": "*Rollout strategy*\nHow much production traffic gets the new build first." },
            "accessory": {
                "type": "static_select",
                "action_id": "strategy",
                "placeholder": label("Pick a strategy"),
                "initial_option": option("Canary 10%", "canary10"),
                "options": [option("Canary 10%", "canary10"), option("Canary 50%", "canary50"), option("Full rollout", "full")]
            }
        }),
        json!({
            "type": "actions",
            "block_id": "decide",
            "elements": [
                { "type": "button", "action_id": "approve", "style": "primary", "text": label("Approve"), "value": "approve" },
                { "type": "button", "action_id": "reject", "style": "danger", "text": label("Reject"), "value": "reject" },
                { "type": "button", "action_id": "logs", "text": label("View logs"), "url": "https://grafana.acme.example/logs/api" },
                { "type": "users_select", "action_id": "reviewer", "placeholder": label("Assign reviewer") },
                { "type": "overflow", "action_id": "more", "options": [option("Re-run checks", "rerun"), option("Pause pipeline", "pause"), option("Open runbook", "runbook")] }
            ]
        }),
        json!({
            "type": "context",
            "elements": [
                { "type": "image", "image_url": CI_ICON, "alt_text": "CI" },
                { "type": "mrkdwn", "text": "Build *#1842* · :white_check_mark: 312 tests passed · triggered by Deploy Bot" }
            ]
        }),
        json!({ "type": "image", "title": label("Staging p99 latency, last 24h"), "image_url": LATENCY_CHART, "alt_text": "Latency chart" }),
        json!({
            "type": "input",
            "block_id": "notes",
            "optional": true,
            "label": label("Release notes"),
            "element": { "type": "plain_text_input", "action_id": "notes", "multiline": true, "placeholder": label("What changed in this release?") },
            "hint": label("Posted to #general after the rollout.")
        }),
        json!({
            "type": "input",
            "block_id": "checks",
            "label": label("Pre-flight checks"),
            "element": {
                "type": "checkboxes",
                "action_id": "checks",
                "options": [option("Migrations reviewed", "migrations"), option("Feature flags set", "flags"), option("On-call notified", "oncall")],
                "initial_options": [option("Migrations reviewed", "migrations")]
            }
        }),
        json!({
            "type": "input",
            "block_id": "window",
            "label": label("Deploy window"),
            "element": {
                "type": "radio_buttons",
                "action_id": "window",
                "options": [option("Now", "now"), option("Tonight, 22:00", "tonight"), option("Tomorrow morning", "tomorrow")],
                "initial_option": option("Tonight, 22:00", "tonight")
            }
        }),
        json!({
            "type": "input",
            "block_id": "freeze",
            "label": label("Code freeze ends"),
            "element": { "type": "datepicker", "action_id": "freeze", "initial_date": "2026-11-23", "placeholder": label("Select a date") }
        }),
    ];
    let mut blocks: Vec<Block> = blocks.into_iter().map(Block::from_value).collect();
    blocks.push(Block::RichText(RichTextBlock {
        block_id: None,
        elements: vec![section(vec![
            plain("React with "),
            emoji("thumbsup", "1f44d"),
            plain(" to vote on the window."),
        ])],
    }));

    let field = |title: &str, value: &str, short: bool| AttachmentField {
        title: Some(title.into()),
        value: Some(value.into()),
        short,
    };
    let attachments = vec![
        Attachment {
            color: Some("#2EB67D".into()),
            pretext: Some("Previous release".into()),
            author_name: Some("GitHub Actions".into()),
            title: Some("acme/api v2.4.3".into()),
            title_link: Some("https://github.com/acme/api/releases/tag/v2.4.3".into()),
            text: Some("Hotfix for the */search* p99 regression :bug:".into()),
            fields: vec![
                field("Status", "Deployed", true),
                field("Duration", "6m 12s", true),
                field(
                    "Changelog",
                    "Add the missing index on the search_terms join; bump reqwest.",
                    false,
                ),
            ],
            footer: Some("deploys".into()),
            ts: Some(json!(base_ts() + DAY + 1_200)),
            ..Default::default()
        },
        Attachment {
            color: Some("warning".into()),
            title: Some("1 flaky test quarantined".into()),
            text: Some("`search::fts::ranking_is_stable` failed 2 of 50 runs.".into()),
            ..Default::default()
        },
    ];

    Message {
        ts: ts(offset),
        channel: Some("C0ENG".into()),
        bot_id: Some("B0DEPLOY".into()),
        username: Some("Deploy Bot".into()),
        subtype: Some(MessageSubtype::BotMessage),
        text: "Release v2.5.0 is ready for production".into(),
        blocks,
        attachments,
        reactions: vec![
            reaction("partyparrot", &["U0ALICE", "U0CARA"]),
            reaction("+1::skin-tone-3", &["U0DAN"]),
        ],
        ..Default::default()
    }
}

fn image_file(id: &str, name: &str, size: i64, width: i64, height: i64) -> File {
    File {
        id: id.into(),
        name: Some(name.into()),
        title: Some(name.into()),
        mimetype: Some("image/png".into()),
        filetype: Some("png".into()),
        pretty_type: Some("PNG".into()),
        size: Some(size),
        original_w: Some(width),
        original_h: Some(height),
        ..Default::default()
    }
}

fn doc_file(id: &str, name: &str, filetype: &str, pretty_type: &str, size: i64) -> File {
    File {
        id: id.into(),
        name: Some(name.into()),
        title: Some(name.into()),
        filetype: Some(filetype.into()),
        pretty_type: Some(pretty_type.into()),
        size: Some(size),
        ..Default::default()
    }
}

fn user(id: &str, name: &str, real_name: &str) -> User {
    User {
        id: id.into(),
        name: name.into(),
        real_name: Some(real_name.into()),
        profile: Profile {
            display_name: Some(name.into()),
            real_name: Some(real_name.into()),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn channel(id: &str, name: &str, topic: &str, private: bool, members: u32) -> Conversation {
    Conversation {
        num_members: Some(members),
        id: id.into(),
        name: Some(name.into()),
        is_channel: !private,
        is_group: private,
        is_private: private,
        is_member: true,
        topic: Some(TopicPurpose {
            value: topic.into(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// A plain (mrkdwn-only) channel message `offset` seconds after [`base_ts()`].
fn msg(channel: &str, offset: u64, user: &str, text: &str) -> Message {
    Message {
        ts: ts(offset),
        channel: Some(channel.into()),
        user: Some(user.into()),
        text: text.into(),
        ..Default::default()
    }
}

/// A rich-text message; `text` stays as the mrkdwn fallback.
fn msg_rich(
    channel: &str,
    offset: u64,
    user: &str,
    text: &str,
    containers: Vec<RichTextContainer>,
) -> Message {
    let mut message = msg(channel, offset, user, text);
    message.blocks = vec![Block::RichText(RichTextBlock {
        block_id: None,
        elements: containers,
    })];
    message
}

fn ts(offset: u64) -> Ts {
    Ts::new(format!("{}.000100", base_ts() + offset))
}

fn reaction(name: &str, users: &[&str]) -> Reaction {
    Reaction {
        name: name.into(),
        count: users.len() as u32,
        users: users.iter().map(|&u| u.into()).collect(),
    }
}

fn section(elements: Vec<RichTextElement>) -> RichTextContainer {
    RichTextContainer::Section(RichTextSection { elements })
}

fn plain(text: &str) -> RichTextElement {
    RichTextElement::Text(TextElement {
        text: text.into(),
        style: None,
    })
}

fn styled(text: &str, style: TextStyle) -> RichTextElement {
    RichTextElement::Text(TextElement {
        text: text.into(),
        style: Some(style),
    })
}

fn mention(user_id: &str) -> RichTextElement {
    RichTextElement::User(UserElement {
        user_id: user_id.into(),
        style: None,
    })
}

fn link(url: &str, text: &str) -> RichTextElement {
    RichTextElement::Link(LinkElement {
        url: url.into(),
        text: Some(text.into()),
        style: None,
    })
}

fn emoji(name: &str, unicode: &str) -> RichTextElement {
    RichTextElement::Emoji(EmojiElement {
        name: name.into(),
        unicode: Some(unicode.into()),
        ..Default::default()
    })
}

fn code_block(language: &str, code: &str) -> RichTextContainer {
    RichTextContainer::Preformatted(RichTextPreformatted {
        elements: vec![plain(code)],
        language: Some(language.into()),
        ..Default::default()
    })
}

fn bullet_list(items: &[&str]) -> RichTextContainer {
    RichTextContainer::List(RichTextList {
        style: ListStyle::Bullet,
        elements: items
            .iter()
            .map(|&item| section(vec![plain(item)]))
            .collect(),
        indent: None,
        offset: None,
        border: None,
    })
}
