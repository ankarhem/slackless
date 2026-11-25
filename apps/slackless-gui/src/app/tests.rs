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

    // 4. Opening a thread from All Unreads opens the right-hand ThreadPanel WITHOUT
    //    leaving Unreads, AND navigating from Unreads records Unreads in Back history.
    let (unreads_entity, eng_thread_ts) = cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.show_unreads(window, cx);
            assert!(app.unreads_open);
            let ts = app
                .fixtures
                .messages
                .get(&ChannelId::from("C0ENG"))
                .and_then(|msgs| msgs.iter().find(|m| m.reply_count.unwrap_or(0) > 0))
                .map(|m| m.ts.clone())
                .expect("engineering has a thread root");
            (app.unreads.clone(), ts)
        })
    });
    cx.update(|_, cx| {
        unreads_entity.update(cx, |_, cx| {
            cx.emit(UnreadsEvent::OpenThread {
                channel: ChannelId::from("C0ENG"),
                thread_ts: eng_thread_ts.clone(),
            });
        });
    });
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        app.read_with(cx, |app, cx| {
            assert!(
                app.unreads_open,
                "clicking replies in Unreads must keep the Unreads panel open"
            );
            assert!(
                app.thread_open,
                "clicking replies in Unreads must open the right-hand thread panel"
            );
            assert_eq!(
                app.thread.read(cx).open_thread(),
                Some((ChannelId::from("C0ENG"), eng_thread_ts)),
            );
        });
    });

    // Navigating to a conversation from Unreads must push `NavPlace::Unreads` onto Back.
    cx.update(|_, cx| {
        unreads_entity.update(cx, |_, cx| {
            cx.emit(UnreadsEvent::OpenConversation(ChannelId::from("C0ENG")));
        });
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            assert!(!app.unreads_open);
            assert_eq!(
                app.current_place(cx),
                NavPlace::Channel(ChannelId::from("C0ENG"))
            );
            app.go_back(window, cx);
            assert!(
                app.unreads_open,
                "Back button after navigating from Unreads must return to Unreads"
            );
            assert_eq!(app.current_place(cx), NavPlace::Unreads);
        });
        let _ = window.draw(cx);
    });
}

#[gpui::test]
fn dm_unread_clears_when_viewed_and_stays_read(cx: &mut gpui::TestAppContext) {
    use gpui_kit::component::Root;
    use slackless_core::api::ClientCounts;
    use std::cell::RefCell;

    cx.update(|cx| {
        cx.set_global(crate::zoom::Zoom::default());
        gpui_kit::init(cx);
        theme::init(cx);
    });
    let fx = fixtures::load();
    let mounted = Rc::new(RefCell::new(None));
    let slot = mounted.clone();
    let (_root, cx) = cx.add_window_view(move |window, cx| {
        let app = App::new(Arc::new(fx), CoreBridge::offline(), window, cx);
        *slot.borrow_mut() = Some(app.clone());
        Root::new(app, window, cx)
    });
    let app = mounted.borrow_mut().take().unwrap();
    cx.run_until_parked();

    // Switch fixtures to live-like mode (`sample = false`) with an unread DM carrying `dm_count`.
    let (general_id, dm_id) = cx.update(|window, cx| {
        let mut general_id = ChannelId::from("C0GENERAL");
        let mut dm_id = ChannelId::default();
        app.update(cx, |app, cx| {
            let mut live_like = (*app.fixtures).with_counts(HashMap::new());
            live_like.sample = false;
            live_like.messages.clear();
            general_id = live_like.channels[0].id.clone();
            dm_id = live_like.dms[0].id.clone();
            app.apply_snapshot(Arc::new(live_like), window, cx);
            app.sidebar
                .update(cx, |sidebar, cx| sidebar.set_activity_unread(0, cx));
            app.select(general_id.clone(), cx);

            // 1. Server `client.counts` reports unread DM via `dm_count` (even if `has_unreads` is false).
            let counts = ClientCounts {
                ims: vec![ConversationCounts {
                    id: dm_id.clone(),
                    last_read: Some(Ts::from("1700000100.000100")),
                    latest: Some(Ts::from("1700000200.000100")),
                    mention_count: 0,
                    dm_count: 2,
                    has_unreads: false,
                    extra: Default::default(),
                }],
                ..Default::default()
            };
            app.on_core_event(&Event::Counts(Box::new(counts)), window, cx);
            assert!(
                app.sidebar.read(cx).unread_ids().contains(&dm_id),
                "DM with dm_count > 0 must show unread in sidebar"
            );
            assert_eq!(app.sidebar.read(cx).badge_count(), 1);
            assert!(
                crate::unreads::local_groups(&app.fixtures, std::slice::from_ref(&dm_id))
                    .iter()
                    .any(|g| g.channel == dm_id),
                "unread DM must appear in All Unreads"
            );

            // 2. Open the DM and simulate stale SQLite history (`newest = 100`, behind `latest = 200`):
            //    `mark_read` must advance cursor to `latest = 200` and sync `fixtures` + sidebar.
            app.select(dm_id.clone(), cx);
            app.mark_read(&dm_id, Ts::from("1700000100.000100"), cx);
            assert_eq!(
                app.ws
                    .counts
                    .get(&dm_id)
                    .and_then(|c| c.last_read.as_ref())
                    .map(Ts::as_str),
                Some("1700000200.000100")
            );
            assert!(
                !app.ws.counts.get(&dm_id).unwrap().is_unread(),
                "ws.counts must clear unread state on viewed DM"
            );
            assert_eq!(
                app.fixtures
                    .conversation(&dm_id)
                    .and_then(|c| c.last_read.as_ref())
                    .map(Ts::as_str),
                Some("1700000200.000100"),
                "fixtures.conversation.last_read must advance so re-entering clears New divider"
            );

            // 3. Switch away from the DM: it must STAY read in sidebar, badge, and All Unreads.
            app.select(general_id.clone(), cx);
            assert!(
                !app.sidebar.read(cx).unread_ids().contains(&dm_id),
                "viewed DM must not become unread again after switching away"
            );
            assert_eq!(app.sidebar.read(cx).badge_count(), 0);
            assert!(
                crate::unreads::local_groups(&app.fixtures, std::slice::from_ref(&dm_id))
                    .is_empty(),
                "viewed DM must disappear from All Unreads"
            );

            // 4. Stale in-flight `client.counts` (same `latest = 200`) must NOT resurrect unreads.
            let stale_counts = ClientCounts {
                ims: vec![ConversationCounts {
                    id: dm_id.clone(),
                    last_read: Some(Ts::from("1700000100.000100")),
                    latest: Some(Ts::from("1700000200.000100")),
                    mention_count: 2,
                    dm_count: 2,
                    has_unreads: true,
                    extra: Default::default(),
                }],
                ..Default::default()
            };
            app.on_core_event(&Event::Counts(Box::new(stale_counts)), window, cx);
            assert!(
                !app.sidebar.read(cx).unread_ids().contains(&dm_id),
                "stale in-flight client.counts must not resurrect unread on already-viewed DM"
            );

            // 5. A new live DM arrives via RTM (`ts = 300`) while viewing `#general`:
            //    it must light up as unread again (clearing `sidebar.read`), and then viewing
            //    it must mark `300` read cleanly.
            let incoming = slackless_core::types::Message {
                ts: Ts::from("1700000300.000100"),
                user: Some(UserId::from("U0OTHER")),
                text: "hey there".to_owned(),
                ..Default::default()
            };
            app.on_core_event(
                &Event::MessagePosted {
                    channel: dm_id.clone(),
                    message: Box::new(incoming),
                },
                window,
                cx,
            );
            assert!(
                app.sidebar.read(cx).unread_ids().contains(&dm_id),
                "live incoming DM must show unread even after earlier mark_read"
            );
            assert_eq!(app.sidebar.read(cx).badge_count(), 1);

            // 6. An `im_marked` read marker from another device clears the unread DM cleanly.
            app.on_core_event(
                &Event::ReadMarker {
                    channel: dm_id.clone(),
                    ts: Ts::from("1700000300.000100"),
                },
                window,
                cx,
            );
            assert!(
                !app.sidebar.read(cx).unread_ids().contains(&dm_id),
                "Event::ReadMarker (im_marked) must clear unread DM"
            );
            assert_eq!(app.sidebar.read(cx).badge_count(), 0);
            assert_eq!(
                app.fixtures
                    .conversation(&dm_id)
                    .and_then(|c| c.last_read.as_ref())
                    .map(Ts::as_str),
                Some("1700000300.000100")
            );
        });
        let _ = window.draw(cx);
        (general_id, dm_id)
    });
    assert_ne!(general_id, dm_id);
}

#[gpui::test]
fn native_huddle_dock_invite_banner_and_audio_preferences_work_end_to_end(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(gpui_kit::init);
    cx.update(crate::theme::init);
    cx.update(|cx| cx.set_global(crate::zoom::Zoom::default()));
    let fixtures = Arc::new(fixtures::load());
    let general = ChannelId::from("C0GENERAL");
    let design = ChannelId::from("C0DESIGN");
    let inviter = fixtures
        .users
        .iter()
        .find(|u| u.id != fixtures.self_user)
        .expect("other user in fixtures")
        .id
        .clone();

    let mounted: Rc<std::cell::RefCell<Option<gpui::Entity<App>>>> =
        Rc::new(std::cell::RefCell::new(None));
    let slot = mounted.clone();
    let win = cx.add_window(|window, cx| {
        let app = App::new(fixtures.clone(), CoreBridge::offline(), window, cx);
        *slot.borrow_mut() = Some(app.clone());
        gpui_kit::component::Root::new(app, window, cx)
    });
    let app = mounted.borrow_mut().take().unwrap();
    let mut cx = gpui::VisualTestContext::from_window(win.into(), cx);
    cx.run_until_parked();

    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            // 1. Start huddle in #general -> persistent sidebar mini-dock mounts.
            app.select(general.clone(), cx);
            app.start_huddle(window, cx);
            assert!(app.huddle_dock.is_some(), "sidebar mini-dock should mount");
            assert_eq!(
                app.huddle_session.as_ref().map(|s| &s.channel),
                Some(&general)
            );

            // 2. Switching channels keeps the mini-dock pinned in the sidebar.
            app.select(design.clone(), cx);
            assert!(
                app.huddle_dock.is_some(),
                "sidebar mini-dock must persist across channel switches"
            );

            // 3. Mute toggle updates session + dock state.
            let was_muted = app.huddle_session.as_ref().unwrap().muted;
            app.toggle_huddle_mute(cx);
            assert_eq!(app.huddle_session.as_ref().unwrap().muted, !was_muted);
            assert_eq!(
                app.huddle_dock.as_ref().unwrap().read(cx).session().muted,
                !was_muted
            );

            // 4. Live speaking / RMS audio levels render without error.
            app.on_core_event(
                &Event::HuddleAudioLevels(slackless_core::huddle::HuddleAudioLevels {
                    local_rms: 0.25,
                    active_speakers: vec![(inviter.clone(), 0.72)],
                    muted_peers: Vec::new(),
                }),
                window,
                cx,
            );

            // 4b. Phase 2–4: video frames (camera & screen share) and DATA_MESSAGE events
            // (reactions, hand raise, topic, captions) flow into HuddleDock -> HuddleViewState.
            let frame = slackless_core::huddle::VideoFrame::from_bgra(4, 4, vec![180u8; 4 * 4 * 4]);
            app.on_core_event(
                &Event::HuddleVideoFrame {
                    tile: slackless_core::huddle::VideoTileKey::LocalCamera,
                    frame: frame.clone(),
                },
                window,
                cx,
            );
            app.on_core_event(
                &Event::HuddleVideoFrame {
                    tile: slackless_core::huddle::VideoTileKey::RemoteScreen(inviter.clone()),
                    frame,
                },
                window,
                cx,
            );
            app.on_core_event(
                &Event::HuddleDataEvent(slackless_core::huddle::HuddleDataEvent::Reaction {
                    user: inviter.clone(),
                    emoji: "confetti".to_owned(),
                    kind: slackless_core::huddle::HuddleReactionKind::Effect,
                    emoji_url: None,
                    at_ms: 0,
                }),
                window,
                cx,
            );
            app.on_core_event(
                &Event::HuddleDataEvent(slackless_core::huddle::HuddleDataEvent::HandRaise {
                    user: inviter.clone(),
                    raised: true,
                }),
                window,
                cx,
            );
            app.on_core_event(
                &Event::HuddleDataEvent(slackless_core::huddle::HuddleDataEvent::StickyEmoji {
                    user: inviter.clone(),
                    emoji: Some("coffee".to_owned()),
                }),
                window,
                cx,
            );
            app.on_core_event(
                &Event::HuddleDataEvent(slackless_core::huddle::HuddleDataEvent::Draw {
                    user: inviter.clone(),
                    message: slackless_core::huddle::HuddleDrawMessage::ChangeHue {
                        item_id: inviter.to_string(),
                        hue: 135,
                    },
                    at_ms: 0,
                }),
                window,
                cx,
            );
            app.on_core_event(
                &Event::HuddleDataEvent(slackless_core::huddle::HuddleDataEvent::Draw {
                    user: inviter.clone(),
                    message: slackless_core::huddle::HuddleDrawMessage::New {
                        item_id: inviter.to_string(),
                        segment: slackless_core::huddle::HuddleDrawSegment {
                            a: [0.1, 0.2],
                            b: [0.2, 0.3],
                            c: [0.3, 0.4],
                            d: [0.4, 0.5],
                        },
                    },
                    at_ms: 0,
                }),
                window,
                cx,
            );
            app.on_core_event(
                &Event::HuddleDataEvent(slackless_core::huddle::HuddleDataEvent::Draw {
                    user: inviter.clone(),
                    message: slackless_core::huddle::HuddleDrawMessage::Position {
                        item_id: inviter.to_string(),
                        x: 0.4,
                        y: 0.5,
                    },
                    at_ms: 0,
                }),
                window,
                cx,
            );
            app.on_core_event(
                &Event::HuddleDataEvent(slackless_core::huddle::HuddleDataEvent::Jukebox {
                    user: inviter.clone(),
                    song_key: Some("lofi".to_owned()),
                }),
                window,
                cx,
            );
            app.on_core_event(
                &Event::HuddleDataEvent(slackless_core::huddle::HuddleDataEvent::Topic {
                    user: inviter.clone(),
                    topic: "Release readiness".to_owned(),
                }),
                window,
                cx,
            );
            app.on_core_event(
                &Event::HuddleDataEvent(slackless_core::huddle::HuddleDataEvent::Caption {
                    user: inviter.clone(),
                    text: "Shipping full parity…".to_owned(),
                    is_partial: true,
                    at_ms: 0,
                }),
                window,
                cx,
            );
            app.on_core_event(
                &Event::HuddleDataEvent(slackless_core::huddle::HuddleDataEvent::Caption {
                    user: inviter.clone(),
                    text: "Shipping full parity today!".to_owned(),
                    is_partial: false,
                    at_ms: 0,
                }),
                window,
                cx,
            );
            let vs = app.huddle_dock.as_ref().unwrap().read(cx).view_state();
            let video_store = app.huddle_dock.as_ref().unwrap().read(cx).video_store();
            assert_eq!(video_store.read(cx).frames().len(), 2);
            assert!(vs.raised_hands.contains(&inviter));
            assert_eq!(
                vs.sticky_emojis.get(&inviter).map(String::as_str),
                Some("coffee")
            );
            assert_eq!(vs.reactions.len(), 1);
            assert!(vs.active_effect.is_some());
            assert_eq!(vs.draw_strokes.len(), 1);
            assert_eq!(vs.draw_strokes[0].hue, 135);
            assert!(vs.draw_cursors.contains_key(&inviter));
            assert_eq!(vs.session.jukebox_song.as_deref(), Some("lofi"));
            assert_eq!(vs.session.topic.as_deref(), Some("Release readiness"));
            // Partial caption updated in-place by final caption -> 1 row!
            assert_eq!(vs.captions.len(), 1);
            assert_eq!(vs.captions[0].1, "Shipping full parity today!");
            assert!(!vs.captions[0].2);
            app.on_core_event(
                &Event::HuddleVideoTileClosed(slackless_core::huddle::VideoTileKey::LocalCamera),
                window,
                cx,
            );
            assert_eq!(video_store.read(cx).frames().len(), 1);

            // 5. Incoming DM huddle invite banner mounts and cancels cleanly.
            app.on_core_event(
                &Event::HuddleInvite {
                    channel: design.clone(),
                    inviter: inviter.clone(),
                    call_id: "R0INVITE1".to_owned(),
                },
                window,
                cx,
            );
            assert!(
                app.huddle_invite.is_some(),
                "incoming DM huddle invite should mount floating ring banner"
            );
            app.on_core_event(
                &Event::HuddleInviteCancelled {
                    channel: Some(design.clone()),
                    call_id: None,
                },
                window,
                cx,
            );
            assert!(
                app.huddle_invite.is_none(),
                "huddle_invite_cancel should dismiss floating ring banner"
            );

            // 6. Preferences -> Audio & video section renders and releases mic probe on exit.
            app.preferences
                .update(cx, |p, cx| p.show(Section::AudioVideo, cx));
        });
        let _ = window.draw(cx);
        app.update(cx, |app, cx| {
            app.preferences
                .update(cx, |p, cx| p.show(Section::Notifications, cx));
            // 7. Leaving clears session and unmounts sidebar mini-dock.
            app.leave_huddle(cx);
            assert!(app.huddle_dock.is_none());
            assert!(app.huddle_session.is_none());
        });
        let _ = window.draw(cx);
    });
}

#[gpui::test]
fn qa_image_viewer_keyboard_zoom_step_and_zero_subscription_leak(cx: &mut gpui::TestAppContext) {
    use gpui_kit::base::Root;
    use std::cell::RefCell;

    cx.update(|cx| {
        cx.set_global(crate::zoom::Zoom::default());
        gpui_kit::init(cx);
        theme::init(cx);
    });
    let mounted = Rc::new(RefCell::new(None));
    let slot = mounted.clone();
    let (_root, cx) = cx.add_window_view(move |window, cx| {
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
    });

    let baseline_subs = app.read_with(cx, |app, _| app._subscriptions.len());
    let sample_images = || {
        let img = |name: &str| ViewerImage {
            name: name.to_owned(),
            source: gpui::ImageSource::Image(Arc::new(gpui::Image::from_bytes(
                gpui::ImageFormat::Png,
                Vec::new(),
            ))),
            user: None,
            taken_at: None,
            download: None,
        };
        #[allow(clippy::arc_with_non_send_sync)]
        Arc::new(vec![img("one.png"), img("two.png")])
    };

    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let ch = app.channel.clone();
            app.open_image_viewer(ch, sample_images(), 0, window, cx);
        });
        let _ = window.draw(cx);
    });
    assert_eq!(
        app.read_with(cx, |app, _| app._subscriptions.len()),
        baseline_subs,
        "opening ImageViewer must not append to App._subscriptions"
    );

    // Step forward/back, zoom in/out/reset, then close with Escape.
    cx.simulate_keystrokes("right");
    cx.run_until_parked();
    assert_eq!(
        app.read_with(cx, |app, cx| app
            .image_viewer
            .as_ref()
            .unwrap()
            .read(cx)
            .current_index()),
        1
    );

    let fit_zoom = app.read_with(cx, |app, cx| {
        app.image_viewer.as_ref().unwrap().read(cx).zoom_index()
    });
    cx.simulate_keystrokes("=");
    cx.run_until_parked();
    assert_eq!(
        app.read_with(cx, |app, cx| app
            .image_viewer
            .as_ref()
            .unwrap()
            .read(cx)
            .zoom_index()),
        fit_zoom + 1,
        "`=` should zoom in"
    );
    cx.simulate_keystrokes("0");
    cx.run_until_parked();
    assert_eq!(
        app.read_with(cx, |app, cx| app
            .image_viewer
            .as_ref()
            .unwrap()
            .read(cx)
            .zoom_index()),
        fit_zoom,
        "`0` should reset zoom to fit"
    );
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(app.read_with(cx, |app, _| app.image_viewer.is_none()));
    assert!(app.read_with(cx, |app, _| app._viewer_sub.is_none()));
}

#[gpui::test]
fn qa_personal_todo_and_major_views_render_cleanly(cx: &mut gpui::TestAppContext) {
    use gpui_kit::base::Root;
    use std::cell::RefCell;

    cx.update(|cx| {
        cx.set_global(crate::zoom::Zoom::default());
        gpui_kit::init(cx);
        theme::init(cx);
    });
    let mounted = Rc::new(RefCell::new(None));
    let slot = mounted.clone();
    let (_root, cx) = cx.add_window_view(move |window, cx| {
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

    let initial_channel = app.read_with(cx, |app, _| app.channel.clone());

    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            // Add a personal to-do and emit LaterEvent::Open with the sentinel channel:
            // App must ignore it rather than switching to a nonexistent `"TODO"` channel.
            app.open_later(cx);
            app.later.update(cx, |later, cx| {
                later.add_todo("Review QA checklist".into(), cx)
            });
        });
        let _ = window.draw(cx);

        app.update(cx, |app, cx| {
            // Walk every top-level view and draw each frame to catch layout/render regressions.
            app.open_unreads(window, cx);
        });
        let _ = window.draw(cx);

        app.update(cx, |app, cx| {
            app.open_threads(cx);
        });
        let _ = window.draw(cx);

        app.update(cx, |app, cx| {
            app.open_drafts(cx);
        });
        let _ = window.draw(cx);

        app.update(cx, |app, cx| {
            app.open_activity(cx);
        });
        let _ = window.draw(cx);

        app.update(cx, |app, cx| {
            app.open_directory(cx);
        });
        let _ = window.draw(cx);

        app.update(cx, |app, cx| {
            app.open_files(cx);
        });
        let _ = window.draw(cx);

        app.update(cx, |app, cx| {
            app.open_huddles(cx);
        });
        let _ = window.draw(cx);

        app.update(cx, |app, cx| {
            app.open_home(cx);
            assert_eq!(app.channel, initial_channel);

            // Populate Preferences -> Advanced with an available release + notes highlights.
            let info = crate::updater::ReleaseInfo {
                version: "2026.11.24-99".to_owned(),
                html_url: "https://github.com/ankarhem/slackless/releases/tag/2026.11.24-99"
                    .to_owned(),
                asset_name: "Slackless-2026.11.24-99-macos-arm64.app.tar.gz".to_owned(),
                download_url: "https://example.com/archive.tar.gz".to_owned(),
                sha256_url: Some("https://example.com/SHA256SUMS.txt".to_owned()),
                notes: vec![
                    "Add cross-workspace search".to_owned(),
                    "Keep scroll smooth over hover popovers".to_owned(),
                ],
            };
            app.preferences.update(cx, |prefs, cx| {
                prefs.set_update_state(
                    crate::updater::UpdateState::Available(info.clone()),
                    Some(info),
                    cx,
                );
                prefs.show(Section::Advanced, cx);
            });
        });
        let _ = window.draw(cx);
    });
}

fn mount_test_app(
    fx: Fixtures,
    cx: &mut gpui::TestAppContext,
) -> (
    gpui::Entity<gpui_kit::base::Root>,
    gpui::Entity<App>,
    &mut gpui::VisualTestContext,
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
        let app = App::new(Arc::new(fx), CoreBridge::offline(), window, cx);
        *slot.borrow_mut() = Some(app.clone());
        Root::new(app, window, cx)
    });
    let app = mounted.borrow_mut().take().unwrap();
    cx.run_until_parked();
    (root, app, cx)
}

#[gpui::test]
fn should_notify_and_system_notification_dedup_cover_all_rules(cx: &mut gpui::TestAppContext) {
    let mut fx = fixtures::load();
    let me = fx.self_user.clone();
    let peer = fx
        .users
        .iter()
        .find(|u| u.id != me && !u.is_bot)
        .expect("peer user")
        .id
        .clone();
    let eng = ChannelId::from("C0ENG");
    let general = ChannelId::from("C0GENERAL");
    let dm = fx.dms[0].id.clone();
    fx.notify_levels
        .insert(eng.clone(), NotifyLevel::Everything);
    fx.notify_levels.insert(general.clone(), NotifyLevel::Muted);

    let (root, app, cx) = mount_test_app(fx, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            // Ensure app is viewing a different channel than eng/general/dm.
            app.select(ChannelId::from("C0DESIGN"), cx);

            let plain_msg = |user: &UserId, text: &str| Message {
                ts: Ts::from("1700000100.000100"),
                user: Some(user.clone()),
                text: text.to_owned(),
                ..Default::default()
            };

            // 1. Desktop notifications disabled -> never notifies.
            app.preferences.update(cx, |p, cx| {
                let mut s = p.settings().clone();
                s.desktop_notifications = false;
                p.load_settings(s, window, cx);
            });
            assert!(!app.should_notify(&dm, &plain_msg(&peer, "hi"), window, cx));

            // Re-enable desktop notifications with global notify_level = 1 (Mentions).
            app.preferences.update(cx, |p, cx| {
                let mut s = p.settings().clone();
                s.desktop_notifications = true;
                s.notify_level = 1;
                p.load_settings(s, window, cx);
            });

            // 2. Own message never notifies.
            assert!(!app.should_notify(&dm, &plain_msg(&me, "my own msg"), window, cx));

            // 3. Per-channel NotifyLevel::Everything overrides global Mentions (1) and Nothing (2).
            assert!(
                app.should_notify(&eng, &plain_msg(&peer, "regular update"), window, cx),
                "per-channel Everything must notify even under global Mentions"
            );
            app.preferences.update(cx, |p, cx| {
                let mut s = p.settings().clone();
                s.notify_level = 2;
                p.load_settings(s, window, cx);
            });
            assert!(
                app.should_notify(&eng, &plain_msg(&peer, "regular update"), window, cx),
                "per-channel Everything must notify even under global Nothing"
            );
            assert!(
                !app.should_notify(
                    &ChannelId::from("C0RANDOM"),
                    &plain_msg(&peer, &format!("hi <@{me}>")),
                    window,
                    cx
                ),
                "global Nothing suppresses channels without override"
            );

            // 4. Muted channel suppresses normal messages and mentions unless breakthrough.
            app.preferences.update(cx, |p, cx| {
                let mut s = p.settings().clone();
                s.notify_level = 0; // All
                p.load_settings(s, window, cx);
            });
            assert!(!app.should_notify(&general, &plain_msg(&peer, "muted"), window, cx));

            // 5. DND / Focus mode blocks normal DMs, but VIP and urgent DM override break through
            //    (including muted channels).
            app.self_state.paused_until = Some(crate::messages::now_secs() + 3600);
            assert!(!app.should_notify(&dm, &plain_msg(&peer, "normal dm"), window, cx));

            app.preferences.update(cx, |p, cx| {
                let mut s = p.settings().clone();
                s.urgent_dm_override = true;
                p.load_settings(s, window, cx);
            });
            assert!(
                app.should_notify(&dm, &plain_msg(&peer, "[URGENT] prod down"), window, cx),
                "urgent DM breaks through DND"
            );

            app.preferences.update(cx, |p, cx| {
                let mut s = p.settings().clone();
                s.urgent_dm_override = false;
                s.vip_users = peer.as_str().to_owned();
                p.load_settings(s, window, cx);
            });
            assert!(
                app.should_notify(&general, &plain_msg(&peer, "vip ping"), window, cx),
                "VIP sender breaks through DND and muted channel"
            );

            // Clear DND + VIP for remaining checks.
            app.self_state.paused_until = None;
            app.preferences.update(cx, |p, cx| {
                let mut s = p.settings().clone();
                s.vip_users.clear();
                s.notify_level = 0;
                s.thread_replies = true;
                p.load_settings(s, window, cx);
            });

            // 6. On-screen suppression when window is active.
            window.activate_window();
            app.select(eng.clone(), cx);
            if window.is_window_active() {
                assert!(
                    !app.should_notify(&eng, &plain_msg(&peer, "on screen"), window, cx),
                    "active channel on screen must not notify"
                );
            }
            app.select(ChannelId::from("C0DESIGN"), cx);

            // 7. Thread reply rules: parent_user_id == me notifies when thread_replies=true,
            //    non-parent reply suppressed unless channel is in follow_all_threads_channels.
            let mut reply_to_me = plain_msg(&peer, "following up");
            reply_to_me.thread_ts = Some(Ts::from("1700000001.000100"));
            reply_to_me.parent_user_id = Some(me.clone());
            assert!(app.should_notify(&eng, &reply_to_me, window, cx));

            let mut reply_to_other = reply_to_me.clone();
            reply_to_other.parent_user_id = Some(peer.clone());
            assert!(!app.should_notify(&eng, &reply_to_other, window, cx));

            app.preferences.update(cx, |p, cx| {
                let mut s = p.settings().clone();
                s.follow_all_threads_channels = vec![eng.to_string()];
                p.load_settings(s, window, cx);
            });
            assert!(app.should_notify(&eng, &reply_to_other, window, cx));

            // 8. system_notification_kind 60s duplicate-tag dedup.
            app.recent_notified_tags.clear();
            app.system_notification_kind("msg:C0ENG:1.1", "T", "B", false, cx);
            assert_eq!(app.recent_notified_tags.len(), 1);
            app.system_notification_kind("msg:C0ENG:1.1", "T", "B", false, cx);
            assert_eq!(
                app.recent_notified_tags.len(),
                1,
                "duplicate tag within 60s must be ignored"
            );
            app.system_notification_kind("msg:C0ENG:1.2", "T", "B", false, cx);
            assert_eq!(app.recent_notified_tags.len(), 2);
        });
    });
    drop(root);
}

#[gpui::test]
fn message_posted_typing_cleanup_thread_reply_rules_and_multi_workspace_sync_badge(
    cx: &mut gpui::TestAppContext,
) {
    let mut fx = fixtures::load();
    fx.sample = false;
    fx.messages.clear();
    fx.counts.clear();
    let peer = fx
        .users
        .iter()
        .find(|u| u.id != fx.self_user && !u.is_bot)
        .unwrap()
        .id
        .clone();
    let me = fx.self_user.clone();
    let general = ChannelId::from("C0GENERAL");
    let eng = ChannelId::from("C0ENG");
    let dm = fx.dms[0].id.clone();

    let (root, app, cx) = mount_test_app(fx, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.select(general.clone(), cx);
            app.sidebar.update(cx, |s, cx| s.set_activity_unread(0, cx));

            // 1. Typing indicator clears when author posts a message in that channel.
            app.someone_typing(&eng, &peer, None, cx);
            assert!(app.typing.contains_key(&(eng.clone(), None)));
            let msg = Message {
                ts: Ts::from("1700000100.000100"),
                user: Some(peer.clone()),
                text: format!("ping <@{me}>"),
                ..Default::default()
            };
            app.message_posted(&eng, &msg, window, cx);
            assert!(
                !app.typing.contains_key(&(eng.clone(), None)),
                "posting clears sender typing indicator"
            );
            let eng_counts = app.ws.counts.get(&eng).expect("counts updated");
            assert!(eng_counts.has_unreads);
            assert_eq!(eng_counts.mention_count, 1);

            // 2. Non-broadcast thread reply does NOT bump channel unread/mention counts,
            //    while ThreadBroadcast DOES.
            let plain_reply = Message {
                ts: Ts::from("1700000105.000100"),
                thread_ts: Some(Ts::from("1700000100.000100")),
                user: Some(peer.clone()),
                text: format!("quiet reply <@{me}>"),
                ..Default::default()
            };
            app.message_posted(&eng, &plain_reply, window, cx);
            assert_eq!(app.ws.counts.get(&eng).unwrap().mention_count, 1);

            let broadcast_reply = Message {
                ts: Ts::from("1700000110.000100"),
                thread_ts: Some(Ts::from("1700000100.000100")),
                subtype: Some(slackless_core::types::MessageSubtype::ThreadBroadcast),
                user: Some(peer.clone()),
                text: format!("also to channel <@{me}>"),
                ..Default::default()
            };
            app.message_posted(&eng, &broadcast_reply, window, cx);
            assert_eq!(app.ws.counts.get(&eng).unwrap().mention_count, 2);

            // 3. Incoming DM bumps dm_count and mention_count.
            let dm_msg = Message {
                ts: Ts::from("1700000120.000100"),
                user: Some(peer.clone()),
                text: "hey".into(),
                ..Default::default()
            };
            app.message_posted(&dm, &dm_msg, window, cx);
            let dm_counts = app.ws.counts.get(&dm).expect("dm counts");
            assert_eq!(dm_counts.dm_count, 1);
            assert_eq!(dm_counts.mention_count, 1);

            // 4. Multi-workspace sync_badge aggregates active + bg_counts + bg_activity_unread.
            if app.bg_counts.len() < 2 {
                app.bg_counts.resize_with(2, HashMap::new);
            }
            if app.bg_activity_unread.len() < 2 {
                app.bg_activity_unread.resize(2, 0);
            }
            app.bg_counts[1].insert(
                ChannelId::from("C_BG"),
                ConversationCounts {
                    id: ChannelId::from("C_BG"),
                    has_unreads: true,
                    mention_count: 3,
                    ..Default::default()
                },
            );
            app.bg_activity_unread[1] = 2;

            app.sync_badge(cx);
            // Active sidebar has 1 unread DM (count=1) + bg workspace has 3 mentions + 2 activity = 6.
            assert_eq!(app.badge, Some(6));
            assert_eq!(app.badge_label, Some(Some("6".to_owned())));

            // Clearing mentions while leaving channel unread + badge_on_unreads = true shows bullet.
            app.bg_counts[1].clear();
            app.bg_activity_unread[1] = 0;
            app.mark_read(&dm, Ts::from("1700000120.000100"), cx);
            app.preferences.update(cx, |p, cx| {
                let mut s = p.settings().clone();
                s.show_dock_badge = true;
                s.badge_on_unreads = true;
                p.load_settings(s, window, cx);
            });
            app.sync_badge(cx);
            assert_eq!(app.badge, Some(0));
            assert_eq!(app.badge_label, Some(Some("\u{2022}".to_owned())));

            // Disabling show_dock_badge clears label completely.
            app.preferences.update(cx, |p, cx| {
                let mut s = p.settings().clone();
                s.show_dock_badge = false;
                p.load_settings(s, window, cx);
            });
            app.sync_badge(cx);
            assert_eq!(app.badge, Some(0));
            assert_eq!(app.badge_label, Some(None));
        });
    });
    drop(root);
}

#[gpui::test]
fn run_slash_broadcast_confirmation_and_draft_persistence_across_channels(
    cx: &mut gpui::TestAppContext,
) {
    let mut fx = fixtures::load();
    // Give C0GENERAL >= 6 members and C0RANDOM < 6 members so we test both sides of threshold.
    for ch in &mut fx.channels {
        if ch.id.as_str() == "C0GENERAL" {
            ch.num_members = Some(10);
        } else if ch.id.as_str() == "C0RANDOM" {
            ch.num_members = Some(3);
        }
    }
    let dm = fx.dms[0].id.clone();

    let (root, app, cx) = mount_test_app(fx, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            // 1. Slash commands: /shrug, /away, /status clear, /dnd 30m, /dnd off, invalid /dnd.
            app.select(ChannelId::from("C0RANDOM"), cx);
            let before_count = app.messages.read(cx).message_count_for_test(cx);
            let shrug = crate::slash::parse("/shrug nice").unwrap();
            app.run_slash(&shrug, None, window, cx);
            assert_eq!(
                app.messages.read(cx).message_count_for_test(cx),
                before_count + 1
            );
            assert!(
                app.messages
                    .read(cx)
                    .last_message_text_for_test(cx)
                    .unwrap()
                    .ends_with(SHRUG)
            );

            assert!(!app.self_state.away);
            app.run_slash(&crate::slash::parse("/away").unwrap(), None, window, cx);
            assert!(app.self_state.away);
            app.run_slash(&crate::slash::parse("/away").unwrap(), None, window, cx);
            assert!(!app.self_state.away);

            app.self_state.set_status(Some(crate::status::UserStatus {
                emoji: "coffee".into(),
                text: "Break".into(),
                clear_after: 0,
                pause: false,
                expiration: None,
            }));
            app.run_slash(
                &crate::slash::parse("/status clear").unwrap(),
                None,
                window,
                cx,
            );
            assert!(app.self_state.status.is_none());

            app.run_slash(&crate::slash::parse("/dnd 30m").unwrap(), None, window, cx);
            assert!(app.self_state.paused_until.is_some());
            let prev_until = app.self_state.paused_until;
            app.run_slash(
                &crate::slash::parse("/dnd invalid_arg").unwrap(),
                None,
                window,
                cx,
            );
            assert_eq!(
                app.self_state.paused_until, prev_until,
                "invalid /dnd usage leaves pause state unchanged"
            );
            app.run_slash(&crate::slash::parse("/dnd off").unwrap(), None, window, cx);
            assert!(app.self_state.paused_until.is_none());

            // 2. Broadcast confirmation threshold: >= 6 members prompts instead of sending immediately;
            //    < 6 members and DMs send directly.
            let broadcast = crate::composer::content_of("<!here> attention", &app.fixtures);

            app.select(ChannelId::from("C0GENERAL"), cx);
            let gen_before = app.messages.read(cx).message_count_for_test(cx);
            app.send(broadcast.clone(), window, cx);
            assert_eq!(
                app.messages.read(cx).message_count_for_test(cx),
                gen_before,
                "channel with >= 6 members must open confirmation dialog instead of sending immediately"
            );
            window.close_dialog(cx);

            app.select(ChannelId::from("C0RANDOM"), cx);
            let rand_before = app.messages.read(cx).message_count_for_test(cx);
            app.send(broadcast.clone(), window, cx);
            assert_eq!(
                app.messages.read(cx).message_count_for_test(cx),
                rand_before + 1,
                "channel with < 6 members sends directly"
            );

            app.select(dm.clone(), cx);
            let dm_before = app.messages.read(cx).message_count_for_test(cx);
            app.send(broadcast, window, cx);
            assert_eq!(
                app.messages.read(cx).message_count_for_test(cx),
                dm_before + 1,
                "DM with @here sends directly without threshold prompt"
            );

            // 3. save_draft / load_draft across channel switches.
            app.select(ChannelId::from("C0ENG"), cx);
            app.composer
                .update(cx, |c, cx| c.load_draft("draft for eng", cx));
            app.save_draft(cx);
            assert_eq!(app.drafts.read(cx).draft_count(), 1);

            app.select(ChannelId::from("C0DESIGN"), cx);
            assert_eq!(app.composer.read(cx).text(), "");
            app.composer
                .update(cx, |c, cx| c.load_draft("draft for design", cx));
            app.select(ChannelId::from("C0ENG"), cx);
            assert_eq!(app.composer.read(cx).text(), "draft for eng");
            assert_eq!(app.drafts.read(cx).draft_count(), 2);

            // Clearing draft removes it from DraftsView.
            app.composer.update(cx, |c, cx| c.load_draft("", cx));
            app.save_draft(cx);
            assert_eq!(app.drafts.read(cx).draft_count(), 1);
        });
    });
    drop(root);
}

#[gpui::test]
fn deep_links_permalinks_workspace_switch_focus_leave_and_hover_card_lifecycle(
    cx: &mut gpui::TestAppContext,
) {
    let fx = fixtures::load();
    let team_id = fx.workspaces[0].id.clone();
    let peer = fx
        .users
        .iter()
        .find(|u| u.id != fx.self_user && !u.is_bot)
        .unwrap()
        .id
        .clone();
    let eng_thread_ts = fx
        .messages
        .get(&ChannelId::from("C0ENG"))
        .and_then(|msgs| msgs.iter().find(|m| m.reply_count.unwrap_or(0) > 0))
        .map(|m| m.ts.clone())
        .expect("C0ENG has thread root");

    let (root, app, cx) = mount_test_app(fx, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            // 1. handle_deep_link variants
            assert!(app.handle_deep_link(
                &format!("slack://channel?team={team_id}&id=C0ENG"),
                window,
                cx
            ));
            assert_eq!(app.channel, ChannelId::from("C0ENG"));

            assert!(app.handle_deep_link(
                &format!("slack://user?team={team_id}&id={peer}"),
                window,
                cx
            ));
            assert!(app.profile_open);

            assert!(app.handle_deep_link(
                &format!("slack://open?team={team_id}&id=C0DESIGN"),
                window,
                cx
            ));
            assert_eq!(app.channel, ChannelId::from("C0DESIGN"));

            assert!(app.handle_deep_link(
                &format!("https://app.slack.com/client/{team_id}/C0ENG/thread/C0ENG-1700000001.000100?thread_ts={eng_thread_ts}"),
                window,
                cx
            ));
            assert_eq!(app.channel, ChannelId::from("C0ENG"));
            assert!(app.thread_open);
            app.close_thread(cx);

            assert!(app.handle_deep_link(
                &format!("https://app.slack.com/client/{team_id}/C0GENERAL"),
                window,
                cx
            ));
            assert_eq!(app.channel, ChannelId::from("C0GENERAL"));

            assert!(!app.handle_deep_link("https://example.com/not-slack", window, cx));
            assert!(!app.handle_deep_link(
                &format!("slack://channel?team={team_id}&id=C_NONEXISTENT"),
                window,
                cx
            ));

            // 2. open_link routes Slack permalinks in-app (both channel and thread links).
            app.open_link(
                LinkTarget::Url(SharedString::from(format!(
                    "https://acme.slack.com/archives/C0ENG/p17600000000654321?thread_ts={eng_thread_ts}"
                ))),
                window,
                cx,
            );
            assert_eq!(app.channel, ChannelId::from("C0ENG"));
            assert!(app.thread_open);
            assert_eq!(
                app.thread.read(cx).open_thread(),
                Some((ChannelId::from("C0ENG"), eng_thread_ts.clone()))
            );
            app.close_thread(cx);

            // 3. switch_workspace preserves active channel & draft on round-trip.
            app.select(ChannelId::from("C0DESIGN"), cx);
            app.composer
                .update(cx, |c, cx| c.load_draft("ws0 draft", cx));
            assert!(app.parked.len() >= 2, "fixture has multiple workspaces");
            app.switch_workspace(1, window, cx);
            assert_eq!(app.workspace, 1);
            assert_eq!(app.composer.read(cx).text(), "");

            app.switch_workspace(0, window, cx);
            assert_eq!(app.workspace, 0);
            assert_eq!(app.channel, ChannelId::from("C0DESIGN"));
            assert_eq!(app.composer.read(cx).text(), "ws0 draft");

            // 4. toggle_focus toggles focus mode cleanly.
            assert!(!app.self_state.focused());
            app.toggle_focus(window, cx);
            assert!(app.self_state.focused());
            app.toggle_focus(window, cx);
            assert!(!app.self_state.focused());

            // 5. leave scrubs channel from back/forward/recent and navigates away when active.
            app.select(ChannelId::from("C0ENG"), cx);
            app.select(ChannelId::from("C0RANDOM"), cx);
            app.leave(ChannelId::from("C0RANDOM"), window, cx);
            assert_ne!(app.channel, ChannelId::from("C0RANDOM"));
            assert!(!app.recent.contains(&ChannelId::from("C0RANDOM")));
            assert!(
                !app.back
                    .contains(&NavPlace::Channel(ChannelId::from("C0RANDOM")))
            );

            // 6. Profile hover-card dwell/grace state machine.
            let trig1 = hover_card::Trigger {
                key: SharedString::from("row-1"),
                user: peer.clone(),
                anchor: gpui::Bounds::default(),
            };
            let trig2 = hover_card::Trigger {
                key: SharedString::from("row-2"),
                user: peer.clone(),
                anchor: gpui::Bounds::default(),
            };
            app.profile_hover_entered(trig1.clone(), cx);
            assert!(app.profile_hover.trigger.is_some());
            assert!(app.profile_hover.card.is_none());

            // Stale dwell from another key is ignored; matching key opens card.
            app.hover_dwell_elapsed(&trig2.key, &peer, cx);
            assert!(app.profile_hover.card.is_none());
            app.hover_dwell_elapsed(&trig1.key, &peer, cx);
            assert!(app.profile_hover.card.as_ref() == Some(&trig1));

            // Stale leave from another key is ignored.
            app.profile_hover_left(&trig2.key, cx);
            assert!(app.profile_hover.trigger.as_ref() == Some(&trig1));

            // Leaving trigger starts grace; entering card cancels grace.
            app.profile_hover_left(&trig1.key, cx);
            assert!(app.profile_hover.trigger.is_none());
            assert!(app.profile_hover.grace.is_some());
            app.hover_card_hovered(true, cx);
            assert!(app.profile_hover.grace.is_none());
            app.hover_grace_elapsed(cx);
            assert!(
                app.profile_hover.card.is_some(),
                "grace must not close while pointer is inside card"
            );

            // Leaving card starts grace; grace elapsed closes card.
            app.hover_card_hovered(false, cx);
            assert!(app.profile_hover.grace.is_some());
            app.hover_grace_elapsed(cx);
            assert!(app.profile_hover.card.is_none());

            // close_hover_card closes immediately even if pointer was inside.
            app.profile_hover_entered(trig1.clone(), cx);
            app.hover_dwell_elapsed(&trig1.key, &peer, cx);
            app.hover_card_hovered(true, cx);
            app.close_hover_card(cx);
            assert!(app.profile_hover.card.is_none());
            assert!(!app.profile_hover.inside);
            assert!(app.profile_hover.dwell.is_none());
            assert!(app.profile_hover.grace.is_none());
        });
    });
    drop(root);
}

#[gpui::test]
fn accessibility_cross_workspace_summary_and_replay_focused_message(cx: &mut gpui::TestAppContext) {
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
        app.update(cx, |app, cx| {
            // Cross-workspace summary names every workspace in the fixture.
            let summary = app.announce_cross_workspace_summary(window, cx);
            assert!(
                summary.contains("workspace"),
                "expected workspace summary, got: {summary:?}"
            );

            // ReplayFocusedMessage formats the focused message using verbosity setting.
            let own_ts = app
                .messages
                .read(cx)
                .own_message_ts_for_test()
                .expect("fixture channel has a message");
            app.messages
                .update(cx, |list, cx| list.set_focused_for_test(&own_ts, cx));

            let standard = app
                .replay_focused_message(window, cx)
                .expect("focused message replays");
            assert!(
                standard.contains("at "),
                "standard replay includes timestamp: {standard:?}"
            );

            // Switch verbosity to Concise (1) and verify timestamp is omitted.
            let mut concise = app.preferences.read(cx).settings().clone();
            concise.message_verbosity = 1;
            app.preferences
                .update(cx, |prefs, cx| prefs.load_settings(concise, window, cx));
            let concise_replay = app
                .replay_focused_message(window, cx)
                .expect("concise replay succeeds");
            assert!(
                concise_replay.len() < standard.len(),
                "concise replay should omit the timestamp prefix: concise={concise_replay:?}, standard={standard:?}"
            );
            assert!(concise_replay.contains(": "));
        });
    });
    drop(root);
}
