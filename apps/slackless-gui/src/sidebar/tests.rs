use super::*;
use gpui_kit::gpui;

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
