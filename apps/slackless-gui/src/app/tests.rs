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

#[test]
fn workspace_state_defaults_cleanly_for_atomic_swap() {
    let mut a = WorkspaceState::default();
    let mut b = WorkspaceState::default();
    a.chosen = true;
    a.offline_queued = 3;
    a.active_canvas = Some("F123".into());
    a.pins.insert(ChannelId::from("C1"), Ok(Vec::new()));

    let prev = mem::replace(&mut a, b);
    b = prev;

    assert!(!a.chosen);
    assert_eq!(a.offline_queued, 0);
    assert!(a.active_canvas.is_none());
    assert!(a.pins.is_empty());

    assert!(b.chosen);
    assert_eq!(b.offline_queued, 3);
    assert_eq!(b.active_canvas.as_deref(), Some("F123"));
    assert!(b.pins.contains_key(&ChannelId::from("C1")));
}

#[gpui::test]
fn mouse_back_and_forward_navigate_across_channels_and_pages(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        cx.set_global(crate::zoom::Zoom::default());
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });
    let fixtures = Arc::new(crate::fixtures::load());
    let app_cell = Rc::new(std::cell::RefCell::new(None));
    let cell = app_cell.clone();
    let (_root, cx) = cx.add_window_view(|window, cx| {
        let app = App::new(fixtures.clone(), CoreBridge::offline(), window, cx);
        *cell.borrow_mut() = Some(app.clone());
        gpui_kit::base::Root::new(app, window, cx)
    });
    let app = app_cell.borrow_mut().take().unwrap();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });

    // Visit: initial channel -> Threads -> Unreads -> #engineering
    let initial = cx.update(|_, cx| app.read(cx).channel.clone());
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_threads(cx);
            app.open_unreads(window, cx);
            app.select(ChannelId::from("C0ENG"), cx);
        });
        let _ = window.draw(cx);
    });
    cx.update(|_, cx| {
        assert_eq!(
            app.read(cx).current_place(cx),
            NavPlace::Channel(ChannelId::from("C0ENG"))
        );
    });

    // Mouse Back button: #engineering -> Unreads -> Threads -> initial channel
    cx.simulate_mouse_down(
        point(px(400.), px(300.)),
        MouseButton::Navigate(NavigationDirection::Back),
        gpui::Modifiers::none(),
    );
    cx.update(|window, cx| {
        assert_eq!(app.read(cx).current_place(cx), NavPlace::Unreads);
        let _ = window.draw(cx);
    });

    cx.simulate_mouse_down(
        point(px(400.), px(300.)),
        MouseButton::Navigate(NavigationDirection::Back),
        gpui::Modifiers::none(),
    );
    cx.update(|window, cx| {
        assert_eq!(app.read(cx).current_place(cx), NavPlace::Threads);
        let _ = window.draw(cx);
    });

    cx.simulate_mouse_down(
        point(px(400.), px(300.)),
        MouseButton::Navigate(NavigationDirection::Back),
        gpui::Modifiers::none(),
    );
    cx.update(|window, cx| {
        assert_eq!(
            app.read(cx).current_place(cx),
            NavPlace::Channel(initial.clone())
        );
        let _ = window.draw(cx);
    });

    // Mouse Forward button: initial -> Threads -> Unreads -> #engineering
    cx.simulate_mouse_down(
        point(px(400.), px(300.)),
        MouseButton::Navigate(NavigationDirection::Forward),
        gpui::Modifiers::none(),
    );
    cx.update(|window, cx| {
        assert_eq!(app.read(cx).current_place(cx), NavPlace::Threads);
        let _ = window.draw(cx);
    });

    cx.simulate_mouse_down(
        point(px(400.), px(300.)),
        MouseButton::Navigate(NavigationDirection::Forward),
        gpui::Modifiers::none(),
    );
    cx.update(|window, cx| {
        assert_eq!(app.read(cx).current_place(cx), NavPlace::Unreads);
        let _ = window.draw(cx);
    });

    cx.simulate_mouse_down(
        point(px(400.), px(300.)),
        MouseButton::Navigate(NavigationDirection::Forward),
        gpui::Modifiers::none(),
    );
    cx.update(|_, cx| {
        assert_eq!(
            app.read(cx).current_place(cx),
            NavPlace::Channel(ChannelId::from("C0ENG"))
        );
    });
}

#[gpui::test]
fn app_home_huddle_threads_and_list_csv_work_end_to_end(cx: &mut gpui::TestAppContext) {
    use crate::blockkit::serde_json::json;
    use gpui_kit::base::Root;
    use std::cell::RefCell;

    cx.update(|cx| {
        cx.set_global(crate::zoom::Zoom::default());
        gpui_kit::init(cx);
        theme::init(cx);
    });

    // Augment sample fixtures with a bot user + bot DM so App Home tab behavior is exercised live.
    let mut fx = fixtures::load();
    let bot_id = UserId::from("U0APPBOT");
    let mut bot = User {
        id: bot_id.clone(),
        name: "deploybot".into(),
        real_name: Some("DeployBot".into()),
        is_bot: true,
        ..Default::default()
    };
    bot.profile.title = Some("Release orchestrator".into());
    fx.users.push(bot);
    let bot_dm = ChannelId::from("D0APPBOT");
    fx.dms.push(Conversation {
        id: bot_dm.clone(),
        is_im: true,
        user: Some(bot_id),
        ..Default::default()
    });

    let mounted = Rc::new(RefCell::new(None));
    let slot = mounted.clone();
    let (_root, cx) = cx.add_window_view(move |window, cx| {
        let app = App::new(Arc::new(fx), CoreBridge::offline(), window, cx);
        *slot.borrow_mut() = Some(app.clone());
        Root::new(app, window, cx)
    });
    let app = mounted.borrow_mut().take().unwrap();
    cx.run_until_parked();

    // 1. Huddle bar thread toggle (`HuddleEvent::OpenThread` -> `toggle_huddle_thread`) on `#general`.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.select(ChannelId::from("C0GENERAL"), cx);
            app.start_huddle(window, cx);
            assert!(
                app.huddle_open,
                "fixture huddle bar should open on #general"
            );
            assert!(!app.thread_open);
            app.toggle_huddle_thread(cx);
            assert!(
                app.thread_open,
                "Huddle thread button should open the thread panel"
            );
        });
        let _ = window.draw(cx);
        app.update(cx, |app, cx| {
            app.toggle_huddle_thread(cx);
            assert!(
                !app.thread_open,
                "second click should toggle the huddle thread closed"
            );
            // Browsing another channel while the huddle bar is open must still open the huddle's own channel thread.
            app.select(ChannelId::from("C0DESIGN"), cx);
            app.toggle_huddle_thread(cx);
            assert_eq!(
                app.thread.read(cx).open_thread().map(|(ch, _)| ch),
                Some(ChannelId::from("C0GENERAL")),
                "Huddle thread must target the huddle's channel even when browsing another channel"
            );
            app.close_thread(cx);
        });
        let _ = window.draw(cx);
    });

    // 1b. Huddles page (`HuddlesView::show_local`) populates ts and renders cleanly.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.show_huddles(cx);
            assert!(app.huddles_open);
        });
        let _ = window.draw(cx);
        app.update(cx, |app, cx| {
            let general_ts = app
                .fixtures
                .messages
                .get(&ChannelId::from("C0GENERAL"))
                .and_then(|msgs| msgs.last())
                .map(|m| m.ts.clone())
                .expect("general has fixture messages");
            app.select(ChannelId::from("C0GENERAL"), cx);
            app.open_thread(ChannelId::from("C0GENERAL"), general_ts, cx);
            assert!(!app.huddles_open);
            assert!(app.thread_open);
            app.close_thread(cx);
        });
        let _ = window.draw(cx);
    });

    // 2. Lists tab (including CSV button) renders cleanly.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.select_tab(ConversationTab::List, cx);
        });
        let _ = window.draw(cx);
    });

    // 3. App Home tab on bot DM: Ctrl+1 selects Home, Ctrl+2 selects Messages, BlockKitState inputs
    //    are prepared, and switching away resets tab to Messages.
    let home_dm = bot_dm.clone();
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.select(home_dm.clone(), cx);
            app.ws.app_homes.insert(
                home_dm.clone(),
                Ok(slackless_core::api::AppHomeResponse {
                    home_tab_enabled: true,
                    messages_tab_enabled: true,
                    home_view: Some(json!({
                        "id": "V0HOME",
                        "app_id": "A0DEPLOY",
                        "bot_id": "B0DEPLOY",
                        "blocks": [
                            { "type": "header", "text": { "type": "plain_text", "text": "Deploy Dashboard" } },
                            { "type": "markdown", "text": "Release **v2.4** is live — see [notes](https://acme.example/v2.4)." },
                            { "type": "alert", "style": "success", "text": { "type": "mrkdwn", "text": "All systems operational." } },
                            {
                                "type": "input",
                                "block_id": "note_blk",
                                "label": { "type": "plain_text", "text": "Release note" },
                                "element": { "type": "plain_text_input", "action_id": "note_act", "initial_value": "Shipped" }
                            }
                        ]
                    })),
                }),
            );
            // Ctrl+1 on a bot DM must pick Home (index 0).
            app.select_conversation_tab(0, cx);
            assert_eq!(app.tab, ConversationTab::Home);
        });
        let _ = window.draw(cx);
        let _ = window.draw(cx);
        app.update(cx, |app, cx| {
            // Ctrl+2 picks Messages (index 1).
            app.select_conversation_tab(1, cx);
            assert_eq!(app.tab, ConversationTab::Messages);
            // Switch back to Home, then navigate to #general -> tab must reset to Messages.
            app.select_conversation_tab(0, cx);
            assert_eq!(app.tab, ConversationTab::Home);
            app.select(ChannelId::from("C0GENERAL"), cx);
            assert_eq!(app.tab, ConversationTab::Messages);
        });
        let _ = window.draw(cx);
    });
}
