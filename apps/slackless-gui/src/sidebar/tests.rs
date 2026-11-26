use super::*;
use gpui_kit::gpui;
use slackless_core::types::rich_text::{Block, RichTextContainer, RichTextElement};
use slackless_core::types::{File, SectionKind, UserId};

use super::dms::dm_preview_text;

#[test]
fn nav_prefs_shows_respects_toggles() {
    let prefs = NavPrefs {
        show_home: false,
        show_dms: true,
        show_activity: false,
        show_files: true,
        show_later: false,
        icons_only: true,
        order: [2, 1, 0, 4, 3],
    };
    assert!(!prefs.shows(NavItem::Home));
    assert!(prefs.shows(NavItem::Dms));
    assert!(!prefs.shows(NavItem::Activity));
    assert!(prefs.shows(NavItem::Files));
    assert!(!prefs.shows(NavItem::Todos));
    assert!(prefs.shows(NavItem::Admin));
    assert_eq!(
        prefs.ordered_items(),
        vec![
            NavItem::Activity,
            NavItem::Dms,
            NavItem::Home,
            NavItem::Todos,
            NavItem::Files,
            NavItem::Agents,
            NavItem::Admin,
        ]
    );
}

#[test]
fn drop_target_and_dm_acceptance_follow_section_kinds() {
    let dm_section = ChannelSection {
        id: "S_DM".into(),
        name: "Direct messages".into(),
        kind: SectionKind::DirectMessages,
        emoji: String::new(),
        next_id: None,
        channel_ids: Vec::new(),
    };
    let custom_section = ChannelSection {
        id: "S_CUSTOM".into(),
        name: "Priority".into(),
        kind: SectionKind::Standard,
        emoji: String::new(),
        next_id: None,
        channel_ids: Vec::new(),
    };
    let channels_key = SharedString::from("channels");
    let dms_key = SharedString::from("dms");
    let custom_key = SharedString::from("S_CUSTOM");

    assert_eq!(drop_target(None, &channels_key), Some(None));
    assert_eq!(drop_target(None, &dms_key), None);
    assert_eq!(drop_target(Some(&dm_section), &dms_key), None);
    assert_eq!(
        drop_target(Some(&custom_section), &custom_key),
        Some(Some("S_CUSTOM".into()))
    );
    assert!(accepts_dms(Some(&custom_section), &custom_key));
    assert!(!accepts_dms(None, &channels_key));
}

/// The DM list previews the raw mrkdwn, never a plain-text preview parsed again: a link typed
/// literally arrives escaped, and re-parsing its unescaped form would open the first address
/// under the second's label.
#[test]
fn dm_preview_is_the_raw_mrkdwn_with_the_file_fallback() {
    let fixtures = crate::fixtures::load();
    let typed = Message {
        text: "&lt;https://evil.example|https://bank.example&gt;".into(),
        ..Default::default()
    };
    assert_eq!(dm_preview_text(&fixtures, &typed), typed.text);
    let shared = Message {
        files: vec![File {
            name: Some("plan.pdf".into()),
            ..Default::default()
        }],
        ..Default::default()
    };
    assert_eq!(
        dm_preview_text(&fixtures, &shared),
        "Shared a file: plan.pdf"
    );
}

#[gpui::test]
fn sidebar_unread_counts_stepping_and_section_jumps(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });
    let mut fixtures = crate::fixtures::load();
    fixtures.messages.clear();
    let first = fixtures.channels[0].id.clone();
    let second = fixtures.channels[1].id.clone();
    let dm = fixtures.dms[0].id.clone();
    fixtures.counts.insert(
        second.clone(),
        slackless_core::api::ConversationCounts {
            id: second.clone(),
            has_unreads: true,
            mention_count: 2,
            ..Default::default()
        },
    );
    fixtures.counts.insert(
        dm.clone(),
        slackless_core::api::ConversationCounts {
            id: dm.clone(),
            has_unreads: true,
            mention_count: 3,
            ..Default::default()
        },
    );
    let fixtures = Arc::new(fixtures);

    let mounted = std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = mounted.clone();
    let init_fixtures = fixtures.clone();
    let init_first = first.clone();
    let (_root, cx) = cx.add_window_view(move |window, cx| {
        let entity = Sidebar::new(init_fixtures.clone(), init_first.clone(), window, cx);
        *slot.borrow_mut() = Some(entity.clone());
        gpui_kit::component::Root::new(entity, window, cx)
    });
    let sidebar = mounted.borrow_mut().take().unwrap();

    // Unread counts, badge_count, and mark_read.
    sidebar.read_with(cx, |s, _| {
        let second_conv = fixtures.conversation(&second).unwrap();
        let unread = s.unread_in(second_conv, None);
        assert_eq!(unread.count, 1);
        assert_eq!(unread.mentions, 2);
        assert!(s.unread_ids().contains(&second));
        assert!(s.unread_ids().contains(&dm));
        assert!(s.has_any_unreads());
        // Dock badge = unread activity (0) + unread DMs (1).
        assert_eq!(s.badge_count(), 1);
    });

    sidebar.update(cx, |s, cx| {
        s.set_activity_unread(4, cx);
        s.mark_read(second.clone(), cx);
    });
    sidebar.read_with(cx, |s, _| {
        assert!(!s.unread_ids().contains(&second));
        assert_eq!(s.badge_count(), 5);
    });

    // `step` and `step_section` walk the laid-out rows cleanly.
    sidebar.read_with(cx, |s, _| {
        *s.rows.borrow_mut() = vec![
            (first.clone(), false),
            (second.clone(), false),
            (dm.clone(), true),
        ];
        assert_eq!(s.step(&first, true, false), Some(second.clone()));
        assert_eq!(s.step(&first, true, true), Some(dm.clone()));
        assert_eq!(s.step(&dm, false, false), Some(second.clone()));
        assert_eq!(s.step(&first, false, false), None);

        // Section jump moves between Channels head (`first`) and DMs head (`dm`) and wraps.
        let next_head = s.step_section(&first, true);
        assert!(next_head.is_some());
        assert_ne!(next_head.as_ref(), Some(&first));
        let prev_head = s.step_section(&dm, false);
        assert!(prev_head.is_some());
    });

    // Muted channels with 0 mentions must never bold Unreads, light dots, or enter unread_ids;
    // muted channels with mentions still surface.
    let mut muted_fixtures = crate::fixtures::load();
    muted_fixtures.messages.clear();
    muted_fixtures.counts.clear();
    let muted_ch = muted_fixtures.channels[1].id.clone();
    let muted_mention_ch = muted_fixtures.channels[2].id.clone();
    muted_fixtures
        .notify_levels
        .insert(muted_ch.clone(), NotifyLevel::Muted);
    muted_fixtures
        .notify_levels
        .insert(muted_mention_ch.clone(), NotifyLevel::Muted);
    muted_fixtures.counts.insert(
        muted_ch.clone(),
        slackless_core::api::ConversationCounts {
            id: muted_ch.clone(),
            has_unreads: true,
            mention_count: 0,
            ..Default::default()
        },
    );
    let muted_fixtures = Arc::new(muted_fixtures);
    sidebar.update(cx, |s, cx| s.set_fixtures(muted_fixtures.clone(), cx));
    sidebar.update(cx, |s, cx| s.set_activity_unread(0, cx));
    sidebar.read_with(cx, |s, _| {
        let conv = muted_fixtures.conversation(&muted_ch).unwrap();
        assert_eq!(s.unread_in(conv, None), Unread::default());
        assert!(s.unread_ids().is_empty());
        assert!(!s.has_any_unreads());
    });
    let mut next_counts = muted_fixtures.counts.clone();
    next_counts.insert(
        muted_mention_ch.clone(),
        slackless_core::api::ConversationCounts {
            id: muted_mention_ch.clone(),
            has_unreads: true,
            mention_count: 1,
            ..Default::default()
        },
    );
    let with_mention = Arc::new(muted_fixtures.with_counts(next_counts));
    sidebar.update(cx, |s, cx| s.set_fixtures(with_mention.clone(), cx));
    sidebar.read_with(cx, |s, _| {
        let conv = with_mention.conversation(&muted_mention_ch).unwrap();
        assert_eq!(
            s.unread_in(conv, None),
            Unread {
                count: 1,
                mentions: 1
            }
        );
        assert_eq!(s.unread_ids(), vec![muted_mention_ch.clone()]);
        assert!(s.has_any_unreads());
    });
}

#[test]
fn team_initials_mentions_user_and_hidden_unreads_pure_helpers() {
    use slackless_core::types::rich_text::{
        ListStyle, RichTextBlock, RichTextDivider, RichTextHeader, RichTextList,
        RichTextPreformatted, RichTextQuote, RichTextSection, TextElement, UserElement,
    };

    assert_eq!(team_initials("Acme Corp"), "AC");
    assert_eq!(team_initials("slackless"), "S");
    assert_eq!(team_initials("One Two Three"), "OT");
    assert_eq!(team_initials("   "), "");

    let me = UserId::from("U0ME");
    let other = UserId::from("U0OTHER");
    let mention_el = RichTextElement::User(UserElement {
        user_id: me.clone(),
        style: None,
    });
    let other_el = RichTextElement::User(UserElement {
        user_id: other.clone(),
        style: None,
    });

    // Raw mrkdwn fallback
    let raw_hit = Message {
        text: "hey <@U0ME> check this".into(),
        ..Default::default()
    };
    assert!(mentions_user(&raw_hit, &me));
    assert!(!mentions_user(&raw_hit, &other));

    // Nested RichText blocks: Section, List, Quote, Preformatted, Header
    let containers = [
        RichTextContainer::Section(RichTextSection {
            elements: vec![mention_el.clone()],
        }),
        RichTextContainer::List(RichTextList {
            style: ListStyle::Bullet,
            elements: vec![RichTextContainer::Section(RichTextSection {
                elements: vec![mention_el.clone()],
            })],
            indent: Some(0),
            offset: None,
            border: None,
        }),
        RichTextContainer::Quote(RichTextQuote {
            elements: vec![mention_el.clone()],
            border: None,
        }),
        RichTextContainer::Preformatted(RichTextPreformatted {
            elements: vec![mention_el.clone()],
            border: None,
            language: None,
        }),
        RichTextContainer::Header(RichTextHeader {
            level: Some(1),
            elements: vec![mention_el],
        }),
    ];
    for container in containers {
        let msg = Message {
            blocks: vec![Block::RichText(RichTextBlock {
                block_id: None,
                elements: vec![container],
            })],
            ..Default::default()
        };
        assert!(mentions_user(&msg, &me));
        assert!(!mentions_user(&msg, &other));
    }

    // Divider / plain text / other user do not match
    let no_hit = Message {
        blocks: vec![Block::RichText(RichTextBlock {
            block_id: None,
            elements: vec![
                RichTextContainer::Divider(RichTextDivider::default()),
                RichTextContainer::Section(RichTextSection {
                    elements: vec![
                        other_el,
                        RichTextElement::Text(TextElement {
                            text: "plain".into(),
                            style: None,
                        }),
                    ],
                }),
            ],
        })],
        ..Default::default()
    };
    assert!(!mentions_user(&no_hit, &me));

    // Unlaid-out ScrollHandle has no item bounds -> (None, None).
    let scroll = ScrollHandle::new();
    let (below, above) = hidden_unreads(&scroll, &[(0, false), (2, true)]);
    assert!(below.is_none() && above.is_none());
}

#[gpui::test]
fn sidebar_section_prefs_round_trip_and_hidden_conversations_classify_filters(
    cx: &mut gpui::TestAppContext,
) {
    use slackless_core::api::ConversationCounts;
    use slackless_core::types::ConversationProperties;

    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });

    let mut fx = crate::fixtures::load();
    fx.messages.clear();
    fx.counts.clear();
    let selected_ch = fx.channels[0].id.clone();
    let unread_ch = fx.channels[1].id.clone();
    let mention_ch = fx.channels[2].id.clone();
    let mut read_ch = Conversation {
        id: ChannelId::from("C_READ"),
        name: Some("quiet".into()),
        is_channel: true,
        ..Default::default()
    };
    let mut dormant_ch = Conversation {
        id: ChannelId::from("C_DORMANT"),
        name: Some("old-stuff".into()),
        is_channel: true,
        ..Default::default()
    };
    let mut props = ConversationProperties::default();
    props.extra.insert(
        "is_dormant".into(),
        crate::blockkit::serde_json::json!(true),
    );
    dormant_ch.properties = Some(props);
    let muted_ch = Conversation {
        id: ChannelId::from("C_MUTED"),
        name: Some("noisy".into()),
        is_channel: true,
        ..Default::default()
    };
    read_ch.is_general = false;
    fx.channels.push(read_ch);
    fx.channels.push(dormant_ch);
    fx.channels.push(muted_ch);

    fx.notify_levels
        .insert(ChannelId::from("C_MUTED"), NotifyLevel::Muted);
    fx.counts.insert(
        unread_ch.clone(),
        ConversationCounts {
            id: unread_ch.clone(),
            has_unreads: true,
            mention_count: 0,
            ..Default::default()
        },
    );
    fx.counts.insert(
        mention_ch.clone(),
        ConversationCounts {
            id: mention_ch.clone(),
            has_unreads: true,
            mention_count: 2,
            ..Default::default()
        },
    );
    fx.counts.insert(
        ChannelId::from("C_MUTED"),
        ConversationCounts {
            id: ChannelId::from("C_MUTED"),
            has_unreads: true,
            mention_count: 0,
            ..Default::default()
        },
    );

    let all_ids = vec![
        selected_ch.clone(),
        unread_ch.clone(),
        mention_ch.clone(),
        ChannelId::from("C_READ"),
        ChannelId::from("C_DORMANT"),
        ChannelId::from("C_MUTED"),
    ];

    let fx = Arc::new(fx);
    let mounted = std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = mounted.clone();
    let init_fx = fx.clone();
    let init_sel = selected_ch.clone();
    let (_root, cx) = cx.add_window_view(move |window, cx| {
        let entity = Sidebar::new(init_fx.clone(), init_sel.clone(), window, cx);
        *slot.borrow_mut() = Some(entity.clone());
        gpui_kit::component::Root::new(entity, window, cx)
    });
    let sidebar = mounted.borrow_mut().take().unwrap();

    let key = SharedString::from("channels");

    // 1. Export / restore section prefs round-trip.
    sidebar.update(cx, |s, cx| {
        s.set_section_filter(key.clone(), SectionFilter::Unreads, cx);
        s.set_section_sort(key.clone(), SectionSort::Priority, cx);
        let exported = s.export_section_prefs();
        assert_eq!(
            exported.filters.get("channels"),
            Some(&SectionFilter::Unreads)
        );
        assert_eq!(exported.sorts.get("channels"), Some(&SectionSort::Priority));

        s.restore_section_prefs(crate::bridge::SectionPrefs::default(), cx);
        assert_eq!(s.section_filter(&key), SectionFilter::ActiveOnly);
        s.restore_section_prefs(exported, cx);
        assert_eq!(s.section_filter(&key), SectionFilter::Unreads);
        assert_eq!(s.section_sort(&key, false), SectionSort::Priority);
    });

    // 2. SectionFilter::Unreads hides read + muted + dormant (selected stays visible).
    sidebar.read_with(cx, |s, _| {
        let h = s.hidden_conversations(&key, &all_ids);
        assert!(!h.read.contains(&selected_ch));
        assert!(!h.read.contains(&unread_ch));
        assert!(!h.read.contains(&mention_ch));
        assert!(h.read.contains(&ChannelId::from("C_READ")));
        assert!(h.read.contains(&ChannelId::from("C_DORMANT")));
        assert!(h.muted.contains(&ChannelId::from("C_MUTED")));
    });

    // 3. SectionFilter::Mentions also hides unread-without-mention into `hidden.unread`.
    sidebar.update(cx, |s, cx| {
        s.set_section_filter(key.clone(), SectionFilter::Mentions, cx);
    });
    sidebar.read_with(cx, |s, _| {
        let h = s.hidden_conversations(&key, &all_ids);
        assert!(h.unread.contains(&unread_ch));
        assert!(!h.unread.contains(&mention_ch));
        assert!(h.read.contains(&ChannelId::from("C_READ")));
        assert!(h.muted.contains(&ChannelId::from("C_MUTED")));
    });

    // 4. SectionFilter::ActiveOnly hides dormant into `hidden.dormant` and muted into `hidden.muted`, keeping active read channels visible.
    sidebar.update(cx, |s, cx| {
        s.set_section_filter(key.clone(), SectionFilter::ActiveOnly, cx);
    });
    sidebar.read_with(cx, |s, _| {
        let h = s.hidden_conversations(&key, &all_ids);
        assert!(h.dormant.contains(&ChannelId::from("C_DORMANT")));
        assert!(h.muted.contains(&ChannelId::from("C_MUTED")));
        assert!(!h.read.contains(&ChannelId::from("C_READ")));
    });

    // 5. Collapsed section hides everything except selected & mentions.
    sidebar.update(cx, |s, _| {
        s.collapsed.insert(key.clone());
    });
    sidebar.read_with(cx, |s, _| {
        let h = s.hidden_conversations(&key, &all_ids);
        assert!(h.unread.contains(&unread_ch));
        assert!(h.read.contains(&ChannelId::from("C_READ")));
        assert!(!h.unread.contains(&mention_ch));
    });
}

#[test]
fn nav_prefs_ordered_items_deduplicates_and_recovers_invalid_indices() {
    // Duplicates and out-of-bounds indices must not panic or drop tabs.
    let prefs = NavPrefs {
        order: [99, 2, 2, 0, 100],
        ..Default::default()
    };
    assert_eq!(
        prefs.ordered_items(),
        vec![
            NavItem::Activity,
            NavItem::Home,
            NavItem::Dms,
            NavItem::Files,
            NavItem::Todos,
            NavItem::Agents,
            NavItem::Admin,
        ]
    );
}

#[gpui::test]
fn sidebar_custom_sections_starring_notify_and_nav_cycling(cx: &mut gpui::TestAppContext) {
    use slackless_core::api::ConversationCounts;
    use slackless_core::types::ConversationProperties;

    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });

    let mut fx = crate::fixtures::load();
    fx.sample = true;
    fx.messages.clear();
    fx.counts.clear();
    fx.sections.clear();
    let ch0 = fx.channels[0].id.clone();
    let ch1 = fx.channels[1].id.clone();
    let ch2 = fx.channels[2].id.clone();
    fx.channels[0].is_general = true;
    let mut general_props = ConversationProperties::default();
    general_props.extra.insert(
        "is_dormant".into(),
        crate::blockkit::serde_json::json!(true),
    );
    fx.channels[0].properties = Some(general_props);
    fx.counts.insert(
        ch1.clone(),
        ConversationCounts {
            id: ch1.clone(),
            has_unreads: true,
            mention_count: 2,
            latest: Some(slackless_core::types::Ts::from("1700000001.000100")),
            ..Default::default()
        },
    );
    let fx = Arc::new(fx);

    let mounted = std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = mounted.clone();
    let init_fx = fx.clone();
    let init_sel = ch0.clone();
    let (_root, cx) = cx.add_window_view(move |window, cx| {
        let entity = Sidebar::new(init_fx.clone(), init_sel.clone(), window, cx);
        *slot.borrow_mut() = Some(entity.clone());
        gpui_kit::component::Root::new(entity, window, cx)
    });
    let sidebar = mounted.borrow_mut().take().unwrap();

    sidebar.update(cx, |s, cx| {
        // #general is never considered dormant even with `is_dormant: true`.
        let general = s.fixtures.conversation(&ch0).unwrap();
        assert!(!s.is_dormant(general));

        // Custom section CRUD: create, create-with-channel, rename, move, delete.
        s.add_section("Alpha".into(), cx);
        let alpha_id = s.section_set()[0].id.clone();
        s.add_section_with_channel("Beta".into(), ch1.clone(), cx);
        let beta_id = s.section_set()[1].id.clone();
        assert!(s.section_set()[1].channel_ids.contains(&ch1));

        s.move_channel(
            ch1.clone(),
            Some(beta_id.clone()),
            Some(alpha_id.clone()),
            cx,
        );
        assert!(s.section_set()[0].channel_ids.contains(&ch1));
        assert!(!s.section_set()[1].channel_ids.contains(&ch1));

        s.rename_section_local(&alpha_id, "Renamed Alpha", cx);
        assert_eq!(s.section_set()[0].name, "Renamed Alpha");

        s.delete_section_local(&beta_id, cx);
        assert_eq!(s.section_set().len(), 1);

        // Local starring toggles cleanly without a server Stars section.
        assert!(!s.is_starred(&ch2));
        s.toggle_star(ch2.clone(), cx);
        assert!(s.is_starred(&ch2));
        s.toggle_star(ch2.clone(), cx);
        assert!(!s.is_starred(&ch2));

        // Section sort defaults: Recency for DMs, Alpha for channels.
        let ch_key = SharedString::from("channels");
        let dm_key = SharedString::from("dms");
        assert_eq!(s.section_sort(&ch_key, false), SectionSort::Alpha);
        assert_eq!(s.section_sort(&dm_key, true), SectionSort::Recency);

        // `active_only = false` switches default filter to `SectionFilter::All`.
        s.active_only = false;
        assert_eq!(s.section_filter(&ch_key), SectionFilter::All);

        // `mute_section` sets muted override; `show_muted` keeps muted visible in `hidden_reason`.
        s.mute_section(std::slice::from_ref(&ch2), cx);
        assert_eq!(s.notify_level(&ch2), Some(NotifyLevel::Muted));
        let ch2_conv = s.fixtures.conversation(&ch2).unwrap().clone();
        assert!(matches!(
            s.hidden_reason(&ch2_conv, Unread::default(), SectionFilter::All, true),
            Some(crate::sidebar_menu::Hidden::Muted)
        ));
        s.set_home_prefs(
            HomePrefs {
                show_muted: true,
                ..Default::default()
            },
            cx,
        );
        assert!(
            s.hidden_reason(&ch2_conv, Unread::default(), SectionFilter::All, true)
                .is_none()
        );

        // `search_unread_counts`, `mark_section_read`, and `mark_unread` round-trip.
        s.set_thread_counts(true, 3, cx);
        let (mentions, threads) = s.search_unread_counts();
        assert_eq!(mentions, vec![(ch1.clone(), 2)]);
        assert_eq!(threads, (true, 3));
        s.mark_section_read(&[ch0.clone(), ch1.clone()], cx);
        assert!(s.unread_ids().is_empty());
        s.mark_unread(&ch1, cx);
        assert_eq!(s.unread_ids(), vec![ch1.clone()]);

        // `cycle_nav` wraps forward & backward skipping hidden tabs.
        s.set_nav_prefs(
            NavPrefs {
                show_home: true,
                show_dms: false,
                show_activity: true,
                show_files: false,
                show_later: true,
                ..Default::default()
            },
            cx,
        );
        assert_eq!(s.active_nav(), NavItem::Home);
        s.cycle_nav(true, cx);
        assert_eq!(s.active_nav(), NavItem::Activity);
        s.cycle_nav(true, cx);
        assert_eq!(s.active_nav(), NavItem::Todos);
        s.cycle_nav(true, cx);
        assert_eq!(s.active_nav(), NavItem::Home);
        s.cycle_nav(false, cx);
        assert_eq!(s.active_nav(), NavItem::Todos);
    });
}

#[gpui::test]
fn custom_sections_list_only_joined_channels_and_dms(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });

    let mut fx = crate::fixtures::load();
    fx.messages.clear();
    fx.counts.clear();
    fx.notify_levels.clear();
    let selected = fx.channels[0].id.clone();
    let channel = fx.channels[1].id.clone();
    let dm = fx.dms[0].id.clone();
    // Ids a section can name that are not joined conversations: a channel the browser
    // lists, an archived one, and one the snapshot does not know.
    let browsed = Conversation {
        id: ChannelId::from("C_BROWSED"),
        name: Some("browsed".into()),
        is_channel: true,
        ..Default::default()
    };
    let archived = ChannelId::from("C_ARCHIVED");
    fx.directory.push(browsed.clone());
    fx.archived.push(Conversation {
        id: archived.clone(),
        name: Some("archived".into()),
        is_channel: true,
        ..Default::default()
    });
    let section = |id: &str, kind: SectionKind, channel_ids: Vec<ChannelId>| ChannelSection {
        id: id.into(),
        name: id.into(),
        kind,
        emoji: String::new(),
        next_id: None,
        channel_ids,
    };
    fx.sections = vec![
        section(
            "S_TEAM",
            SectionKind::Standard,
            vec![
                channel.clone(),
                dm.clone(),
                browsed.id.clone(),
                archived.clone(),
                ChannelId::from("C_GONE"),
            ],
        ),
        section("S_CHANNELS", SectionKind::Channels, Vec::new()),
        section("S_DMS", SectionKind::DirectMessages, Vec::new()),
    ];
    let fx = Arc::new(fx);

    let mounted = std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = mounted.clone();
    let (_root, cx) = cx.add_window_view(move |window, cx| {
        let entity = Sidebar::new(fx.clone(), selected.clone(), window, cx);
        *slot.borrow_mut() = Some(entity.clone());
        gpui_kit::component::Root::new(entity, window, cx)
    });
    let sidebar = mounted.borrow_mut().take().unwrap();
    let laid_out = |cx: &mut gpui::VisualTestContext| {
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        sidebar.read_with(cx, |s, _| {
            s.rows
                .borrow()
                .iter()
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>()
        })
    };
    let count = |rows: &[ChannelId], id: &ChannelId| rows.iter().filter(|row| *row == id).count();

    // The custom section, first in the list, holds the channel and the DM it names - once
    // each, not again under Channels or Direct messages - and none of the other ids.
    let rows = laid_out(cx);
    assert!(rows[..2].contains(&channel) && rows[..2].contains(&dm));
    assert_eq!(count(&rows, &channel), 1);
    assert_eq!(count(&rows, &dm), 1);
    assert_eq!(
        count(&rows, &browsed.id),
        0,
        "a browser channel waits to be joined"
    );
    assert_eq!(
        count(&rows, &archived),
        0,
        "an archived channel never lists"
    );

    // Joined from the browser, the channel lists in the section naming it, once.
    sidebar.update(cx, |s, cx| s.add_channel(browsed.clone(), cx));
    let rows = laid_out(cx);
    assert!(rows[..3].contains(&browsed.id));
    assert_eq!(count(&rows, &browsed.id), 1);
}
