//! Unit & GPUI tests for `App` and its helpers.

use super::chrome::*;
use super::helpers::*;
use super::*;
use gpui_kit::gpui;
use slackless_core::types::User;

#[test]
fn permalinks_parse_into_their_in_app_targets() {
    let message =
        parse_permalink("https://acme.slack.com/archives/C0ENG/p17600000000123456").unwrap();
    assert_eq!(message.channel, ChannelId::from("C0ENG"));
    assert_eq!(message.ts.as_str(), "17600000000.123456");
    assert_eq!(message.thread_ts, None);

    // A thread link names its root in `thread_ts`; DM links carry the channel in `cid`.
    let thread = parse_permalink(
        "https://acme.slack.com/archives/D0ALICE/p17600000000654321?thread_ts=17600000000.123456&cid=D0ALICE",
    )
    .unwrap();
    assert_eq!(thread.channel, ChannelId::from("D0ALICE"));
    assert_eq!(
        thread.thread_ts.as_ref().map(Ts::as_str),
        Some("17600000000.123456")
    );

    // Anything else stays a web URL.
    assert!(parse_permalink("https://github.com/gpui").is_none());
    assert!(parse_permalink("https://acme.slack.com/help").is_none());
    assert!(parse_permalink("https://acme.slack.com/archives/C0ENG").is_none());
    assert!(parse_permalink("https://acme.slack.com/archives/C0ENG/plive").is_none());
}

#[test]
fn includes_line_joins_slacks_member_list() {
    assert_eq!(includes_line(&[]), None);
    assert_eq!(includes_line(&["A".into()]), Some("Includes A".into()));
    assert_eq!(
        includes_line(&["A".into(), "B".into()]),
        Some("Includes A and B".into())
    );
    assert_eq!(
        includes_line(&["A".into(), "B".into(), "C".into()]),
        Some("Includes A, B, and C".into())
    );
    // Slack names three; a longer list never renders but still joins three.
    assert_eq!(
        includes_line(&["A".into(), "B".into(), "C".into(), "D".into()]),
        Some("Includes A, B, and C".into())
    );
}

#[test]
fn channel_people_recent_authors_then_members() {
    let fixtures = fixtures::load();
    let person = |id: &str| User {
        id: UserId::from(id),
        real_name: Some(id.to_owned()),
        ..Default::default()
    };
    let bot = User {
        id: UserId::from("UBOT"),
        is_bot: true,
        ..Default::default()
    };
    let message = |user: Option<&str>, bot_profile: bool| Message {
        user: user.map(UserId::from),
        bot_profile: bot_profile.then(Default::default),
        ..Default::default()
    };
    // Newest last, so the popover reads authors newest first, skipping bots, the
    // signed-in user and repeats, then tops up from the member list.
    let history = vec![
        message(Some("U0ONE"), false),
        message(Some("UBOT"), false),
        message(Some("U0TWO"), false),
        message(None, true),
        message(Some("U0SELF"), false),
        message(Some("U0TWO"), false),
    ];
    let members = ["U0ONE", "U0THREE"].map(UserId::from).to_vec();
    let mut fixtures = fixtures;
    fixtures.self_user = UserId::from("U0SELF");
    fixtures.users = vec![person("U0ONE"), person("U0TWO"), person("U0THREE"), bot];
    let people = channel_people(&fixtures, &members, &history);
    assert_eq!(
        people.iter().map(UserId::as_str).collect::<Vec<_>>(),
        ["U0TWO", "U0ONE", "U0THREE"],
        "newest author first, then member top-up"
    );
}

#[test]
fn notifications_follow_mentions_broadcasts_and_keywords() {
    let me = UserId::from("U1");
    assert!(mentions_me("hi <@U1>", &me, ""));
    assert!(mentions_me("hi <@U1|jakob>", &me, ""));
    assert!(!mentions_me("hi <@U12>", &me, ""));
    assert!(mentions_me("<!here> deploying", &me, ""));
    assert!(mentions_me("The Checkout is down", &me, "deploy, checkout"));
    assert!(!mentions_me("nothing here", &me, "deploy, , checkout"));
}

#[gpui::test]
fn composer_typing_and_editing_are_not_swallowed_by_app_message_shortcuts(
    cx: &mut gpui::TestAppContext,
) {
    use gpui_kit::base::Root;
    use std::cell::RefCell;

    cx.update(|cx| {
        cx.set_global(crate::zoom::Zoom::default());
        gpui_kit::init(cx);
        theme::init(cx);
    });
    let mounted = Rc::new(RefCell::new(None));
    let slot = mounted.clone();
    let (root, cx) = cx.add_window_view(move |window, cx| {
        let app = App::new(
            Arc::new(fixtures::load()),
            CoreBridge::offline(),
            window,
            cx,
        );
        *slot.borrow_mut() = Some(app.clone());
        Root::new(app, window, cx)
    });
    let app = mounted.borrow_mut().take().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        let _ = window.draw(cx);
    });

    // Focus the main composer and type every single-key shortcut letter (e, t, f, p, a, u, m, r)
    // plus Backspace and Right arrow through the full GPUI key dispatch tree.
    cx.update(|window, cx| {
        let composer = app.read(cx).composer.clone();
        composer.update(cx, |composer, cx| composer.focus_input(window, cx));
    });
    cx.run_until_parked();

    cx.simulate_keystrokes("e t f p a u m r");
    cx.run_until_parked();
    let typed = app.read_with(cx, |app, cx| app.composer.read(cx).text());
    assert_eq!(
        typed, "etfpaumr",
        "one-key message shortcuts must not swallow letters typed into the composer"
    );

    // Left, Backspace, Right, and typing at the end all reach the composer's textarea.
    cx.simulate_keystrokes("left backspace right !");
    cx.run_until_parked();
    let edited = app.read_with(cx, |app, cx| app.composer.read(cx).text());
    assert_eq!(
        edited, "etfpaur!",
        "backspace and right arrow must reach the focused composer"
    );

    // Select all + Cmd+B bolds the draft in place.
    cx.simulate_keystrokes("cmd-a cmd-b");
    cx.run_until_parked();
    let bolded = app.read_with(cx, |app, cx| app.composer.read(cx).text());
    assert_eq!(bolded, "*etfpaur!*");

    // Blurring back to the App root restores one-key message shortcuts (`e` opens WYSIWYG edit).
    let own_ts = app.read_with(cx, |app, cx| {
        app.messages
            .read(cx)
            .own_message_ts_for_test()
            .expect("fixture channel has an own message")
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.messages
                .update(cx, |list, cx| list.set_focused_for_test(&own_ts, cx));
            app.focus.focus(window, cx);
        });
    });
    cx.run_until_parked();

    cx.simulate_keystrokes("e");
    cx.run_until_parked();
    let editing = app.read_with(cx, |app, cx| app.messages.read(cx).is_editing_for_test(cx));
    assert!(
        editing,
        "pressing `e` outside an input opens inline WYSIWYG edit"
    );

    // Inside inline edit mode, typing `e, t, f, p, a, u, m, r` + backspace works and Esc cancels.
    cx.simulate_keystrokes("space e t f p a u m r backspace");
    cx.run_until_parked();
    let edit_text = app.read_with(cx, |app, cx| app.messages.read(cx).edit_text_for_test(cx));
    assert!(
        edit_text.ends_with(" etfpaum"),
        "typing in inline edit composer should work, got: {edit_text:?}"
    );

    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let still_editing = app.read_with(cx, |app, cx| app.messages.read(cx).is_editing_for_test(cx));
    assert!(!still_editing, "Esc cancels inline WYSIWYG edit mode");
    drop(root);
}

#[gpui::test]
fn composer_live_preview_collapses_and_reveals_markdown_on_cursor_movement(
    cx: &mut gpui::TestAppContext,
) {
    use gpui_kit::base::Root;
    use std::cell::RefCell;

    cx.update(|cx| {
        cx.set_global(crate::zoom::Zoom::default());
        gpui_kit::init(cx);
        theme::init(cx);
    });
    let mounted = Rc::new(RefCell::new(None));
    let slot = mounted.clone();
    let (root, cx) = cx.add_window_view(move |window, cx| {
        let app = App::new(
            Arc::new(fixtures::load()),
            CoreBridge::offline(),
            window,
            cx,
        );
        *slot.borrow_mut() = Some(app.clone());
        Root::new(app, window, cx)
    });
    let app = mounted.borrow_mut().take().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        let _ = window.draw(cx);
    });

    // Focus composer and type 3 headings + a paragraph with *bold* + trailing space.
    cx.update(|window, cx| {
        let composer = app.read(cx).composer.clone();
        composer.update(cx, |composer, cx| composer.focus_input(window, cx));
    });
    cx.run_until_parked();

    cx.simulate_keystrokes(
        "# space H 1 enter # # space H 2 enter # # # space H 3 enter * b o l d * space",
    );
    cx.run_until_parked();

    let tokens = app.read_with(cx, |app, cx| app.composer.read(cx).token_ids_for_test(cx));
    assert_eq!(
        tokens,
        vec![
            ("h1".into(), "# ".into()),
            ("h2".into(), "## ".into()),
            ("h3".into(), "### ".into()),
            ("hide".into(), "*".into()),
            ("hide".into(), "*".into()),
        ],
        "inactive headings show H1/H2/H3 badges and inactive *bold* hides its `*` markers"
    );

    // Moving Left onto `*bold*` reveals its `*` delimiters while headings stay badges.
    cx.simulate_keystrokes("left");
    cx.run_until_parked();
    let tokens = app.read_with(cx, |app, cx| app.composer.read(cx).token_ids_for_test(cx));
    assert_eq!(
        tokens,
        vec![
            ("h1".into(), "# ".into()),
            ("h2".into(), "## ".into()),
            ("h3".into(), "### ".into()),
        ],
        "caret touching `*bold*` reveals its `*` syntax"
    );

    // Moving Up onto `### H3` reveals `### ` while `*bold*` collapses again and H1/H2 stay badges.
    cx.simulate_keystrokes("up");
    cx.run_until_parked();
    let tokens = app.read_with(cx, |app, cx| app.composer.read(cx).token_ids_for_test(cx));
    assert_eq!(
        tokens,
        vec![
            ("h1".into(), "# ".into()),
            ("h2".into(), "## ".into()),
            ("hide".into(), "*".into()),
            ("hide".into(), "*".into()),
        ],
        "caret on H3 reveals `### ` while H1/H2 stay badges and `*bold*` re-collapses"
    );

    drop(root);
}

#[test]
fn notification_tag_round_trips_channel_and_ts() {
    assert_eq!(
        parse_notification_tag("msg:C0ENG:1700000001.000200"),
        Some((ChannelId::from("C0ENG"), Ts::from("1700000001.000200")))
    );
    assert_eq!(
        parse_notification_tag("later:D123:1700000002.000300"),
        Some((ChannelId::from("D123"), Ts::from("1700000002.000300")))
    );
    assert_eq!(parse_notification_tag("other:C1:1.0"), None);
    assert_eq!(parse_notification_tag("msg:C1:"), None);
}

#[test]
fn broadcast_kind_detects_channel_here_and_ignores_plain() {
    let fixtures = crate::fixtures::load();
    let plain = crate::composer::content_of("hello team", &fixtures);
    assert_eq!(broadcast_kind(&plain), None);

    let here = crate::composer::content_of("<!here> standup", &fixtures);
    assert_eq!(broadcast_kind(&here), Some(BroadcastRange::Here));

    let channel = crate::composer::content_of("> <!channel> deploy", &fixtures);
    assert_eq!(broadcast_kind(&channel), Some(BroadcastRange::Channel));
}

#[test]
fn draft_round_trips_underline_and_usergroup_without_leaking_u_tag_to_mrkdwn() {
    let fixtures = crate::fixtures::load();
    let doc = crate::editor::from_mrkdwn("<u>important</u> ping <!subteam^S0ENG> now", &fixtures);
    assert_eq!(doc.to_draft(), "<u>important</u> ping <!subteam^S0ENG> now");
    assert_eq!(doc.to_mrkdwn(), "important ping <!subteam^S0ENG> now");
    let rt = doc.to_rich_text();
    let round = crate::editor::from_rich_text(&[rt.into()], &fixtures).unwrap();
    assert_eq!(round.to_draft(), doc.to_draft());
}

#[test]
fn workspace_badge_aggregates_mentions_activity_and_unreads() {
    let mut counts = HashMap::new();
    counts.insert(
        ChannelId::from("C1"),
        ConversationCounts {
            id: ChannelId::from("C1"),
            has_unreads: true,
            mention_count: 0,
            ..Default::default()
        },
    );
    assert_eq!(
        workspace_badge_from_counts(&counts, 0),
        WorkspaceBadge {
            mentions: 0,
            has_unreads: true,
        }
    );

    counts.insert(
        ChannelId::from("D1"),
        ConversationCounts {
            id: ChannelId::from("D1"),
            has_unreads: true,
            mention_count: 3,
            ..Default::default()
        },
    );
    assert_eq!(
        workspace_badge_from_counts(&counts, 2),
        WorkspaceBadge {
            mentions: 5,
            has_unreads: true,
        }
    );
}
