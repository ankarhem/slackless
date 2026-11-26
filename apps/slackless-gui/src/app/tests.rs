//! Unit & GPUI tests for `App` and its helpers.

use super::chrome::*;
use super::helpers::*;
use super::*;
use crate::slash::SHRUG;
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

#[test]
fn channel_cache_drops_the_least_recently_loaded_channel() {
    let channel = |ix: usize| ChannelId::from(format!("C{ix}"));
    let mut cache = ChannelCache::default();
    for ix in 0..CHANNEL_CACHE_MAX {
        cache.insert(channel(ix), ix);
    }
    // Loading the oldest channel again makes it the most recent one.
    cache.insert(channel(0), 0);
    cache.insert(channel(CHANNEL_CACHE_MAX), CHANNEL_CACHE_MAX);

    assert!(
        !cache.contains_key(&channel(1)),
        "the least recently loaded channel is dropped"
    );
    assert_eq!(cache.get(&channel(0)), Some(&0), "a reloaded channel stays");
    assert!((2..=CHANNEL_CACHE_MAX).all(|ix| cache.contains_key(&channel(ix))));
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
                app.huddle_state.is_some(),
                "fixture huddle should mount on #general"
            );
            assert!(!app.right_pane.is_thread());
            app.toggle_huddle_thread(cx);
            assert!(
                app.right_pane.is_thread(),
                "Huddle thread button should open the thread panel"
            );
        });
        let _ = window.draw(cx);
        app.update(cx, |app, cx| {
            app.toggle_huddle_thread(cx);
            assert!(
                !app.right_pane.is_thread(),
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
            assert_eq!(app.page, MainPage::Huddles);
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
            assert_ne!(app.page, MainPage::Huddles);
            assert!(app.right_pane.is_thread());
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
            assert_eq!(app.page, MainPage::Unreads);
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
            assert_eq!(
                app.page,
                MainPage::Unreads,
                "clicking replies in Unreads must keep the Unreads panel open"
            );
            assert!(
                app.right_pane.is_thread(),
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
            assert_ne!(app.page, MainPage::Unreads);
            assert_eq!(
                app.current_place(cx),
                NavPlace::Channel(ChannelId::from("C0ENG"))
            );
            app.go_back(window, cx);
            assert_eq!(
                app.page,
                MainPage::Unreads,
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
                app.fixtures.last_read(&dm_id).map(Ts::as_str),
                Some("1700000200.000100"),
                "fixtures.last_read must advance so re-entering clears New divider"
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
                app.fixtures.last_read(&dm_id).map(Ts::as_str),
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

            // 3. Mute toggle updates session + shared HuddleState.
            let was_muted = app.huddle_session.as_ref().unwrap().muted;
            app.toggle_huddle_mute(cx);
            assert_eq!(app.huddle_session.as_ref().unwrap().muted, !was_muted);
            assert_eq!(
                app.huddle_state.as_ref().unwrap().read(cx).session().muted,
                !was_muted
            );

            // 4. Live speaking / RMS audio levels arrive on the dedicated HuddleMediaEvent lane.
            app.on_huddle_media_event(
                &HuddleMediaEvent::AudioLevels(slackless_core::huddle::HuddleAudioLevels {
                    local_rms: 0.25,
                    active_speakers: vec![(inviter.clone(), 0.72)],
                    muted_peers: Vec::new(),
                }),
                window,
                cx,
            );

            // 4b. Phase 2–4: video frames (camera & screen share) arrive on HuddleMediaEvent and
            // DATA_MESSAGE events (reactions, hand raise, topic, captions) arrive on Event::Huddle
            // -> HuddleState -> HuddleViewState.
            let frame = slackless_core::huddle::VideoFrame::from_bgra(4, 4, vec![180u8; 4 * 4 * 4]);
            app.on_huddle_media_event(
                &HuddleMediaEvent::VideoFrame {
                    tile: slackless_core::huddle::VideoTileKey::LocalCamera,
                    frame: frame.clone(),
                },
                window,
                cx,
            );
            app.on_huddle_media_event(
                &HuddleMediaEvent::VideoFrame {
                    tile: slackless_core::huddle::VideoTileKey::RemoteScreen(inviter.clone()),
                    frame,
                },
                window,
                cx,
            );
            app.on_core_event(
                &Event::Huddle(HuddleEvent::Data(
                    slackless_core::huddle::HuddleDataEvent::Reaction {
                        user: inviter.clone(),
                        emoji: "confetti".to_owned(),
                        kind: slackless_core::huddle::HuddleReactionKind::Effect,
                        emoji_url: None,
                        at_ms: 0,
                    },
                )),
                window,
                cx,
            );
            app.on_core_event(
                &Event::Huddle(HuddleEvent::Data(
                    slackless_core::huddle::HuddleDataEvent::HandRaise {
                        user: inviter.clone(),
                        raised: true,
                    },
                )),
                window,
                cx,
            );
            app.on_core_event(
                &Event::Huddle(HuddleEvent::Data(
                    slackless_core::huddle::HuddleDataEvent::StickyEmoji {
                        user: inviter.clone(),
                        emoji: Some("coffee".to_owned()),
                    },
                )),
                window,
                cx,
            );
            app.on_core_event(
                &Event::Huddle(HuddleEvent::Data(
                    slackless_core::huddle::HuddleDataEvent::Draw {
                        user: inviter.clone(),
                        message: slackless_core::huddle::HuddleDrawMessage::ChangeHue {
                            item_id: inviter.to_string(),
                            hue: 135,
                        },
                        at_ms: 0,
                    },
                )),
                window,
                cx,
            );
            app.on_core_event(
                &Event::Huddle(HuddleEvent::Data(
                    slackless_core::huddle::HuddleDataEvent::Draw {
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
                    },
                )),
                window,
                cx,
            );
            app.on_core_event(
                &Event::Huddle(HuddleEvent::Data(
                    slackless_core::huddle::HuddleDataEvent::Draw {
                        user: inviter.clone(),
                        message: slackless_core::huddle::HuddleDrawMessage::Position {
                            item_id: inviter.to_string(),
                            x: 0.4,
                            y: 0.5,
                        },
                        at_ms: 0,
                    },
                )),
                window,
                cx,
            );
            app.on_core_event(
                &Event::Huddle(HuddleEvent::Data(
                    slackless_core::huddle::HuddleDataEvent::Jukebox {
                        user: inviter.clone(),
                        song_key: Some("lofi".to_owned()),
                    },
                )),
                window,
                cx,
            );
            app.on_core_event(
                &Event::Huddle(HuddleEvent::Data(
                    slackless_core::huddle::HuddleDataEvent::Topic {
                        user: inviter.clone(),
                        topic: "Release readiness".to_owned(),
                    },
                )),
                window,
                cx,
            );
            app.on_core_event(
                &Event::Huddle(HuddleEvent::Data(
                    slackless_core::huddle::HuddleDataEvent::Caption {
                        user: inviter.clone(),
                        text: "Shipping full parity…".to_owned(),
                        is_partial: true,
                        at_ms: 0,
                        lang: Some("en".to_owned()),
                    },
                )),
                window,
                cx,
            );
            app.on_core_event(
                &Event::Huddle(HuddleEvent::Data(
                    slackless_core::huddle::HuddleDataEvent::Caption {
                        user: inviter.clone(),
                        text: "Shipping full parity today!".to_owned(),
                        is_partial: false,
                        at_ms: 0,
                        lang: Some("en".to_owned()),
                    },
                )),
                window,
                cx,
            );
            let vs = app.huddle_state.as_ref().unwrap().read(cx).view_state();
            let video_store = app.huddle_state.as_ref().unwrap().read(cx).video_store();
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
            app.on_huddle_media_event(
                &HuddleMediaEvent::VideoTileClosed(
                    slackless_core::huddle::VideoTileKey::LocalCamera,
                ),
                window,
                cx,
            );
            assert_eq!(video_store.read(cx).frames().len(), 1);
            // The ordered-stream copy of a closure (the one a full media lane cannot drop)
            // removes the tile too.
            app.on_core_event(
                &Event::Huddle(HuddleEvent::VideoTileClosed(
                    slackless_core::huddle::VideoTileKey::RemoteScreen(inviter.clone()),
                )),
                window,
                cx,
            );
            assert!(video_store.read(cx).frames().is_empty());

            // 5. Incoming DM huddle invite banner mounts and cancels cleanly.
            app.on_core_event(
                &Event::Huddle(HuddleEvent::Invite {
                    channel: design.clone(),
                    inviter: inviter.clone(),
                    call_id: "R0INVITE1".to_owned(),
                }),
                window,
                cx,
            );
            assert!(
                app.huddle_invite.is_some(),
                "incoming DM huddle invite should mount floating ring banner"
            );
            app.on_core_event(
                &Event::Huddle(HuddleEvent::InviteCancelled {
                    channel: Some(design.clone()),
                    call_id: None,
                }),
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
fn huddle_links_ask_before_joining_then_join_like_the_huddle_button(cx: &mut gpui::TestAppContext) {
    let (_root, app, cx) = mount_test_app(fixtures::load(), cx);
    let design = ChannelId::from("C0DESIGN");
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.preferences.update(cx, |p, cx| {
                let mut s = p.settings().clone();
                s.huddle_join_muted = true;
                p.load_settings(s, window, cx);
            });
            assert!(!window.has_active_dialog(cx));

            // 1. A huddle link (from the OS or in a message) only asks: nothing joins yet.
            assert!(app.handle_deep_link("slack://huddle?team=T0ACME&id=C0DESIGN", window, cx));
            assert_eq!(app.channel, design);
            assert!(
                window.has_active_dialog(cx),
                "a huddle link must ask before joining"
            );
            assert!(
                app.huddle_session.is_none(),
                "a huddle link must not join (microphone live) before the user confirms"
            );

            // 2. Join huddle joins as the huddle button does: Join muted applies.
            window.close_dialog(cx);
            app.start_huddle_in(design.clone(), window, cx);
            let session = app.huddle_session.as_ref().expect("a confirmed link joins");
            assert_eq!(session.channel, design);
            assert!(session.muted, "Join muted applies to a linked huddle");
            app.leave_huddle(cx);

            // 3. In a large channel, confirming still brings the huddle button's member warning.
            app.ws.member_counts.insert(design.clone(), 200);
            assert!(app.handle_deep_link(
                "https://app.slack.com/huddle/T0ACME/C0DESIGN",
                window,
                cx
            ));
            assert!(window.has_active_dialog(cx));
            assert!(app.huddle_session.is_none());
            window.close_dialog(cx);
            app.start_huddle_in(design.clone(), window, cx);
            assert!(
                window.has_active_dialog(cx),
                "the large-channel warning still applies to a linked huddle"
            );
            assert!(app.huddle_session.is_none());
        });
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
            video: None,
            pdf: None,
        };
        std::rc::Rc::new(vec![img("one.png"), img("two.png")])
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

    // Video viewer shortcuts: `c` toggles transcript drawer while `cmd-c` does NOT intercept copy.
    let video_items = std::rc::Rc::new(vec![ViewerImage {
        name: "clip.mp4".into(),
        source: gpui::ImageSource::Image(Arc::new(gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            Vec::new(),
        ))),
        user: None,
        taken_at: None,
        download: None,
        video: Some(crate::image_viewer::ViewerVideo {
            file_id: "F_KEY_VID".into(),
            url: "https://files.slack.com/pri/clip.mp4".into(),
            ext: "mp4".into(),
            duration_ms: Some(15_000),
            inline_transcript: Some("Sample spoken words.".into()),
            vtt_url: None,
        }),
        pdf: None,
    }]);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let ch = app.channel.clone();
            app.open_image_viewer(ch, video_items, 0, window, cx);
        });
        let _ = window.draw(cx);
    });
    assert!(!app.read_with(cx, |app, cx| {
        app.image_viewer
            .as_ref()
            .unwrap()
            .read(cx)
            .showing_transcript()
    }));
    cx.simulate_keystrokes("c");
    cx.run_until_parked();
    assert!(
        app.read_with(cx, |app, cx| app
            .image_viewer
            .as_ref()
            .unwrap()
            .read(cx)
            .showing_transcript()),
        "`c` must open the transcript drawer on a video"
    );
    cx.simulate_keystrokes("cmd-c");
    cx.run_until_parked();
    assert!(
        app.read_with(cx, |app, cx| app
            .image_viewer
            .as_ref()
            .unwrap()
            .read(cx)
            .showing_transcript()),
        "`cmd-c` must NOT toggle the transcript drawer so selected text copy still works"
    );
    cx.simulate_keystrokes("up");
    cx.simulate_keystrokes("down");
    cx.simulate_keystrokes("c");
    cx.run_until_parked();
    assert!(
        !app.read_with(cx, |app, cx| app
            .image_viewer
            .as_ref()
            .unwrap()
            .read(cx)
            .showing_transcript()),
        "second `c` must close the transcript drawer"
    );
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(app.read_with(cx, |app, _| app.image_viewer.is_none()));

    // Floating mini-player / PiP dock renders cleanly when an inline video is playing off-screen.
    cx.update(|window, cx| {
        let ch = app.read(cx).channel.clone();
        crate::media::seed_inline_video_for_test(
            crate::media::InlineVideoMeta {
                file_id: "F_PIP_APP".into(),
                title: "standup-demo.mp4".into(),
                channel: Some(ch),
                message_ts: Some(slackless_core::types::Ts::from("1700000001.000100")),
                url: "https://files.slack.com/pri/standup-demo.mp4".into(),
                ext: "mp4".into(),
                duration_ms: Some(30_000),
                ..Default::default()
            },
            true,
            false,
            cx,
        );
        let _ = window.draw(cx);
        assert!(
            crate::media::active_pip_video(false, cx).is_some(),
            "floating mini-player dock should be active when inline video is off-screen"
        );
        // Switching workspaces cleanly stops any active inline video / PiP from the outgoing workspace.
        app.update(cx, |app, cx| app.switch_workspace(1, window, cx));
        assert!(
            crate::media::active_pip_video(false, cx).is_none(),
            "switching workspaces must stop outgoing inline video / PiP"
        );
    });
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
                is_prerelease: false,
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
    mount_app(fx, vec![CoreBridge::offline()], cx)
}

/// [`mount_test_app`] over `bridges`, one per workspace.
fn mount_app(
    fx: Fixtures,
    bridges: Vec<CoreBridge>,
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
        let app = App::new_multi(Arc::new(fx), bridges, window, cx);
        *slot.borrow_mut() = Some(app.clone());
        Root::new(app, window, cx)
    });
    let app = mounted.borrow_mut().take().unwrap();
    cx.run_until_parked();
    (root, app, cx)
}

/// A workspace signed in as `U0ME` over an in-memory store holding `channels` and `users`,
/// and that store.
fn memory_workspace(team: &str, channels: &[&str], users: &[User]) -> (CoreBridge, SharedStore) {
    let team_id = slackless_core::types::TeamId::from(team);
    let mut store = slackless_core::store::Store::open_in_memory().unwrap();
    let record = Team {
        id: team_id.clone(),
        name: team.to_owned(),
        ..Default::default()
    };
    store.upsert_team(&team_id, &record).unwrap();
    for id in channels {
        let channel = Conversation {
            id: ChannelId::from(*id),
            name: Some(id.to_lowercase()),
            is_channel: true,
            is_member: true,
            ..Default::default()
        };
        store.upsert_conversation(&team_id, &channel).unwrap();
    }
    store.upsert_users(&team_id, users).unwrap();
    let store: SharedStore = Arc::new(std::sync::Mutex::new(store));
    let session = slackless_core::auth::Session {
        team_id,
        user_id: UserId::from("U0ME"),
        workspace_url: format!("https://{}.slack.com", team.to_lowercase()),
        xoxc_token: String::new(),
        d_cookie: String::new(),
    };
    (CoreBridge::with_store(session, store.clone()), store)
}

#[gpui::test]
fn users_changed_patches_people_without_rereading_the_workspace(cx: &mut gpui::TestAppContext) {
    let person = |id: &str, status: &str| User {
        id: UserId::from(id),
        name: id.to_lowercase(),
        profile: slackless_core::types::Profile {
            status_text: Some(status.to_owned()),
            ..Default::default()
        },
        ..Default::default()
    };
    let (bridge, store) = memory_workspace(
        "T0ONE",
        &["C0GENERAL"],
        &[person("U0ME", ""), person("U0ALICE", "lunch")],
    );
    let fx = bridge.snapshot(&HashMap::new()).unwrap();
    let (_root, app, cx) = mount_app(fx, vec![bridge], cx);
    {
        let team = slackless_core::types::TeamId::from("T0ONE");
        let mut store = store.lock().unwrap();
        // Alice's status moves and a stranger is resolved, while a channel lands unannounced:
        // only a re-read of the whole workspace would show it.
        let people = [person("U0ALICE", "in a meeting"), person("U0EXT", "")];
        store.upsert_users(&team, &people).unwrap();
        let channel = Conversation {
            id: ChannelId::from("C0NEW"),
            is_channel: true,
            is_member: true,
            ..Default::default()
        };
        store.upsert_conversation(&team, &channel).unwrap();
    }
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let changed = vec![UserId::from("U0ALICE"), UserId::from("U0EXT")];
            app.on_core_event(&Event::UsersChanged(changed), window, cx);
        })
    });
    // The debounced read lands.
    cx.executor().advance_clock(RELOAD_DEBOUNCE);
    cx.run_until_parked();
    cx.update(|_, cx| {
        let fixtures = &app.read(cx).fixtures;
        let status = |id: &str| {
            fixtures
                .user(&UserId::from(id))
                .and_then(|user| user.profile.status_text.as_deref())
        };
        assert_eq!(status("U0ALICE"), Some("in a meeting"));
        assert_eq!(status("U0EXT"), Some(""), "a newly stored person is added");
        assert_eq!(status("U0ME"), Some(""));
        assert!(
            fixtures
                .conversation(&ChannelId::from("C0GENERAL"))
                .is_some(),
            "the rest of the snapshot is kept"
        );
        assert!(
            fixtures.conversation(&ChannelId::from("C0NEW")).is_none(),
            "a people change must not re-read the whole workspace"
        );
    });
}

#[gpui::test]
fn first_workspace_visit_reads_its_snapshot_off_the_ui_thread_and_drops_a_stale_one(
    cx: &mut gpui::TestAppContext,
) {
    let me = User {
        id: UserId::from("U0ME"),
        ..Default::default()
    };
    let (one, _) = memory_workspace("T0ONE", &["C0ONE"], std::slice::from_ref(&me));
    let (two, _) = memory_workspace("T0TWO", &["C0TWO"], std::slice::from_ref(&me));
    let mut fx = one.snapshot(&HashMap::new()).unwrap();
    fx.workspaces = vec![one.team_summary().unwrap(), two.team_summary().unwrap()];
    let (_root, app, cx) = mount_app(fx, vec![one, two], cx);
    let switch = |cx: &mut gpui::VisualTestContext, index: usize| {
        cx.update(|window, cx| app.update(cx, |app, cx| app.switch_workspace(index, window, cx)));
    };
    let shows = |cx: &mut gpui::VisualTestContext, id: &str| {
        cx.update(|_, cx| {
            app.read(cx)
                .fixtures
                .conversation(&ChannelId::from(id))
                .is_some()
        })
    };

    // The first visit paints at once: its snapshot is not read on the UI thread.
    switch(cx, 1);
    assert!(!shows(cx, "C0TWO"));
    // Leaving before it lands drops it: the workspace switched back to keeps its own.
    switch(cx, 0);
    cx.executor().advance_clock(RELOAD_DEBOUNCE);
    cx.run_until_parked();
    assert!(shows(cx, "C0ONE"));
    assert!(!shows(cx, "C0TWO"));
    // Coming back resumes the read.
    switch(cx, 1);
    cx.executor().advance_clock(RELOAD_DEBOUNCE);
    cx.run_until_parked();
    assert!(shows(cx, "C0TWO"));
    assert!(!shows(cx, "C0ONE"));
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

            // Re-enable desktop notifications with global notify_level = 1 (Mentions) and disable
            // the 9-18 quiet-hours schedule so the test is deterministic at any hour of day.
            app.preferences.update(cx, |p, cx| {
                let mut s = p.settings().clone();
                s.desktop_notifications = true;
                s.schedule_enabled = false;
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
                source: crate::status::StatusSource::Manual,
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
            assert!(app.right_pane.is_profile());

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
            assert!(app.right_pane.is_thread());
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
            assert!(app.right_pane.is_thread());
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

#[gpui::test]
fn wave1_and_wave2_gui_secret_guard_link_scrubber_stealth_and_stt_provider(
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
        app.update(cx, |app, cx| {
            // 1. Pre-send secret guard blocks sending a leaked token and opens confirmation modal.
            let leaked = format!(
                "API_KEY={} rotate me",
                ["xoxb", "1234567890", "abcdefghij123456"].join("-")
            );
            let before_msgs = app.messages.read(cx).message_count_for_test(cx);
            app.send(MessageContent::plain(leaked), window, cx);
            assert_eq!(
                app.messages.read(cx).message_count_for_test(cx),
                before_msgs,
                "secret guard must block send until confirmed"
            );
            window.close_dialog(cx);

            // 2. The send pipeline's tracking-parameter scrubber cleans the text it posts.
            app.send(
                MessageContent::plain(
                    "Check https://example.com/page?utm_source=slack&id=42&fbclid=abc".to_owned(),
                ),
                window,
                cx,
            );
            assert_eq!(
                app.messages
                    .read(cx)
                    .last_message_text_for_test(cx)
                    .as_deref(),
                Some("Check https://example.com/page?id=42")
            );

            // 3. Stealth mode toggle updates settings cleanly.
            app.preferences
                .update(cx, |p, cx| p.set_stealth_mode(true, cx));
            app.apply_preferences(window, cx);
            assert!(app.preferences.read(cx).settings().stealth_mode);
            app.preferences
                .update(cx, |p, cx| p.set_stealth_mode(false, cx));
            app.apply_preferences(window, cx);
            assert!(!app.preferences.read(cx).settings().stealth_mode);

            // 4. Transcription provider select switches cleanly between Slack, Parakeet v3, and Whisper Turbo.
            assert_eq!(
                app.preferences.read(cx).settings().transcription_provider(),
                slackless_core::huddle::TranscriptionProvider::SlackBuiltIn
            );
            app.preferences
                .update(cx, |p, cx| p.set_transcription_provider(1, cx));
            assert_eq!(
                app.preferences.read(cx).settings().transcription_provider(),
                slackless_core::huddle::TranscriptionProvider::ParakeetV3
            );
            app.preferences
                .update(cx, |p, cx| p.set_transcription_provider(2, cx));
            assert_eq!(
                app.preferences.read(cx).settings().transcription_provider(),
                slackless_core::huddle::TranscriptionProvider::WhisperTurbo
            );
        });
    });
    drop(root);
}

#[gpui::test]
fn modal_stack_lifecycle_inline_errors_suggestions_workflows_and_calling_apps(
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
        app.update(cx, |app, cx| {
            use gpui::WeakEntity;
            use slackless_core::api::ViewSubmitResponse;

            // 1. Open root modal V1 -> map server inline errors -> push V2 -> update V2 ->
            //    deliver modal external_select suggestions -> pop back to V1 -> push V3 -> clear all.
            let v1 = crate::blockkit::serde_json::json!({
                "id": "V1",
                "root_view_id": "V1",
                "type": "modal",
                "title": { "type": "plain_text", "text": "Create ticket" },
                "submit": { "type": "plain_text", "text": "Next" },
                "close": { "type": "plain_text", "text": "Cancel" },
                "blocks": [
                    {
                        "type": "input",
                        "block_id": "b_summary",
                        "label": { "type": "plain_text", "text": "Summary" },
                        "element": { "type": "plain_text_input", "action_id": "a_summary" }
                    }
                ]
            });
            app.open_block_modal(v1, window, cx);
            assert_eq!(app.modal_stack.read(cx).depth(), 1);
            assert_eq!(app.modal_stack.read(cx).top_view_id(), Some("V1"));

            // Server inline validation errors map onto b_summary without closing the modal.
            app.handle_modal_submit_result(
                "V1",
                Ok(ViewSubmitResponse::Errors(HashMap::from([(
                    "b_summary".to_owned(),
                    "Summary must start with a ticket prefix.".to_owned(),
                )]))),
                window,
                cx,
            );
            assert_eq!(app.modal_stack.read(cx).depth(), 1);
            let v1_kit = app.modal_stack.read(cx).top_kit().expect("V1 kit");
            assert_eq!(
                v1_kit.read(cx).block_error("b_summary"),
                Some("Summary must start with a ticket prefix.")
            );

            // views.submit response_action: "push" pushes V2 onto the stack.
            let v2 = crate::blockkit::serde_json::json!({
                "id": "V2",
                "root_view_id": "V1",
                "previous_view_id": "V1",
                "type": "modal",
                "title": { "type": "plain_text", "text": "Assign issue" },
                "submit": { "type": "plain_text", "text": "Submit" },
                "blocks": [
                    {
                        "type": "input",
                        "block_id": "b_issue",
                        "label": { "type": "plain_text", "text": "Related issue" },
                        "element": { "type": "external_select", "action_id": "a_issue" }
                    }
                ]
            });
            app.handle_modal_submit_result("V1", Ok(ViewSubmitResponse::Push(v2)), window, cx);
            assert_eq!(app.modal_stack.read(cx).depth(), 2);
            assert_eq!(app.modal_stack.read(cx).top_view_id(), Some("V2"));

            // view_updated updates V2 in place without changing stack depth.
            let v2_upd = crate::blockkit::serde_json::json!({
                "id": "V2",
                "root_view_id": "V1",
                "previous_view_id": "V1",
                "hash": "h2-updated",
                "type": "modal",
                "title": { "type": "plain_text", "text": "Assign issue (updated)" },
                "submit": { "type": "plain_text", "text": "Submit" },
                "blocks": [
                    {
                        "type": "input",
                        "block_id": "b_issue",
                        "label": { "type": "plain_text", "text": "Related issue" },
                        "element": { "type": "external_select", "action_id": "a_issue" }
                    }
                ]
            });
            app.update_block_modal(v2_upd, window, cx);
            assert_eq!(app.modal_stack.read(cx).depth(), 2);
            assert_eq!(
                app.modal_stack.read(cx).frames[1].hash.as_deref(),
                Some("h2-updated")
            );

            // Async blocks.suggestions in modal populates V2's BlockKitState (and ignores
            // prefix-colliding keys like modal-V20/...).
            app.modal_stack.update(cx, |stack, cx| {
                stack.set_suggestions(
                    "modal-V20/b_issue/a_issue",
                    vec![("Wrong".to_owned(), "W".to_owned())],
                    cx,
                );
            });
            app.request_modal_suggestions(
                slackless_core::types::BotId::from("B1"),
                "A1".to_owned(),
                None,
                "V2".to_owned(),
                crate::blockkit::SuggestRequest {
                    key: "modal-V2/b_issue/a_issue".to_owned(),
                    action_id: "a_issue".to_owned(),
                    block_id: "b_issue".to_owned(),
                    value: "ENG".to_owned(),
                    attachment: None,
                },
                cx,
            );

            // Back navigation pops V2 and restores V1.
            app.modal_stack
                .update(cx, |stack, cx| stack.pop_back(window, cx));
            assert_eq!(app.modal_stack.read(cx).depth(), 1);
            assert_eq!(app.modal_stack.read(cx).top_view_id(), Some("V1"));

            // Push V3 then clear all modal views via response_action: "clear".
            let v3 = crate::blockkit::serde_json::json!({
                "id": "V3",
                "root_view_id": "V1",
                "previous_view_id": "V1",
                "type": "modal",
                "title": { "type": "plain_text", "text": "Confirm" },
                "blocks": []
            });
            app.open_block_modal(v3, window, cx);
            assert_eq!(app.modal_stack.read(cx).depth(), 2);
            app.handle_modal_submit_result("V3", Ok(ViewSubmitResponse::Clear), window, cx);
            assert_eq!(app.modal_stack.read(cx).depth(), 0);

            // 2. Workflow details dialog opens cleanly and releases its entity when closed.
            app.open_workflow_details(
                crate::blockkit::WorkflowDetailsRequest {
                    trigger_id: Some("Ft0DEPLOY".to_owned()),
                    url: Some("https://slack.com/shortcuts/Ft0DEPLOY/abc".to_owned()),
                    title: "Deploy production".to_owned(),
                },
                window,
                cx,
            );
            assert!(
                app.workflow_details
                    .as_ref()
                    .and_then(WeakEntity::upgrade)
                    .is_some(),
                "open WorkflowDetailsView dialog must be alive"
            );
            window.close_dialog(cx);
        });
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            use gpui::WeakEntity;
            use slackless_core::domain::calling::CallMode;

            assert!(
                app.workflow_details
                    .as_ref()
                    .and_then(WeakEntity::upgrade)
                    .is_none(),
                "closing WorkflowDetailsView dialog must release the entity"
            );

            // 3. Third-party calling apps catalog + launcher dialog + launch flow.
            let calling_apps = app.calling_apps();
            assert!(
                calling_apps.iter().any(|a| a.id == "zoom")
                    && calling_apps.iter().any(|a| a.id == "meet")
                    && calling_apps.iter().any(|a| a.id == "teams"),
                "sample mode should expose Zoom, Google Meet, and Microsoft Teams"
            );
            app.launch_calling_app(&calling_apps[0], CallMode::Audio, "Incident", window, cx);
            app.open_calling_app_dialog(window, cx);
            window.close_dialog(cx);
        });
    });
    cx.run_until_parked();
    drop(root);
}

#[gpui::test]
fn cross_workspace_search_jump_storage_backfill_progress_and_export_engine(
    cx: &mut gpui::TestAppContext,
) {
    use gpui_kit::base::Root;
    use slackless_core::{BackfillStatus, ChannelBackfillProgress, WorkspaceBackfillProgress};
    use std::cell::RefCell;

    cx.update(|cx| {
        cx.set_global(crate::zoom::Zoom::default());
        gpui_kit::init(cx);
        theme::init(cx);
    });
    let mounted = Rc::new(RefCell::new(None));
    let slot = mounted.clone();
    let (root, cx) = cx.add_window_view(move |window, cx| {
        let fx = Arc::new(fixtures::workspace(0));
        let app = App::new(fx, CoreBridge::offline(), window, cx);
        *slot.borrow_mut() = Some(app.clone());
        Root::new(app, window, cx)
    });
    let app = mounted.borrow_mut().take().unwrap();
    cx.run_until_parked();

    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            assert_eq!(app.workspace, 0);
            assert!(
                app.fixtures.workspaces.len() >= 2,
                "fixture mode should provide at least two workspaces for cross-workspace jump test"
            );

            // 1. Cross-workspace search jump switches workspace and selects the target channel.
            let target_ws_id = app.fixtures.workspaces[1].id.clone();
            let ws1_fx = fixtures::workspace(1);
            let target_ch = ws1_fx.channels[0].id.clone();
            let target_ts = ws1_fx
                .messages
                .get(&target_ch)
                .and_then(|msgs| msgs.first())
                .map(|m| m.ts.clone())
                .unwrap_or_else(|| Ts::from("1700000001.000000"));

            app.open_message_in_workspace(&target_ws_id, target_ch.clone(), target_ts, window, cx);
            assert_eq!(app.workspace, 1);
            assert_eq!(app.channel, target_ch);

            // 2. Event::BackfillProgress updates workspace state, preferences, and storage dialog opens cleanly.
            let progress = WorkspaceBackfillProgress {
                workspace: target_ws_id.clone(),
                status: BackfillStatus::Running,
                paused: false,
                active_channel: Some(target_ch.clone()),
                total_channels: 2,
                completed_channels: 1,
                pages_fetched: 3,
                messages_stored: 120,
                threads_fetched: 2,
                replies_stored: 8,
                channels: vec![ChannelBackfillProgress {
                    channel: target_ch.clone(),
                    channel_name: Some("#general".into()),
                    status: BackfillStatus::Running,
                    pages_fetched: 3,
                    messages_stored: 120,
                    threads_fetched: 2,
                    replies_stored: 8,
                    oldest_ts: None,
                    newest_ts: None,
                    complete: false,
                    last_error: None,
                }],
            };
            app.on_core_event(&Event::BackfillProgress(progress.clone()), window, cx);
            assert_eq!(app.ws.backfill_progress.as_ref(), Some(&progress));

            app.open_storage_dialog(window, cx);
            window.close_dialog(cx);

            // 3. Searching from workspace index 1 works in both CurrentWorkspace and AllWorkspaces
            //    scopes, and jumping back to workspace 0 switches workspaces accurately.
            let ws0_id = app.fixtures.workspaces[0].id.clone();
            let ws0_fx = fixtures::workspace(0);
            let ws0_ch = ws0_fx.channels[0].id.clone();
            let ws0_ts = ws0_fx
                .messages
                .get(&ws0_ch)
                .and_then(|msgs| msgs.first())
                .map(|m| m.ts.clone())
                .unwrap_or_else(|| Ts::from("1700000001.000000"));
            let ws1_word = ws1_fx
                .messages
                .values()
                .flatten()
                .flat_map(|m| m.text.split_whitespace())
                .find(|w| w.len() >= 5 && w.chars().all(|c| c.is_ascii_alphabetic()))
                .expect("workspace 1 should contain a searchable word")
                .to_owned();

            app.run_message_search(ws1_word.clone(), cx);
            let results_entity = app.results.clone();
            assert_eq!(
                results_entity.read(cx).active_workspace_for_test(),
                target_ws_id
            );

            // 4. Fixture channel & thread export on workspace 1 uses workspace 1's ID/name and
            //    includes file transcripts in thread exports.
            let mut fx1_export = fixtures::workspace(1);
            let fid = slackless_core::types::FileId::from("F_CLIP_WS1");
            fx1_export
                .file_transcripts
                .insert(fid.clone(), "Spoken note in ws1 thread".into());
            let export_ch = fx1_export.channels[0].id.clone();
            let root_ts = Ts::from("1700999000.000100");
            fx1_export
                .messages
                .entry(export_ch.clone())
                .or_default()
                .push(Message {
                    ts: root_ts.clone(),
                    text: "Clip root".into(),
                    files: vec![File {
                        id: fid,
                        name: Some("clip.mp4".into()),
                        ..Default::default()
                    }],
                    ..Default::default()
                });
            let ch_doc = super::dialogs::storage::build_fixture_channel_export(
                &fx1_export,
                1,
                &export_ch,
                "#ch",
            );
            assert_eq!(ch_doc.workspace, target_ws_id.as_str());
            assert_eq!(
                ch_doc.workspace_name.as_deref(),
                Some(fx1_export.workspaces[1].name.as_str())
            );
            let th_doc = super::dialogs::storage::build_fixture_thread_export(
                &fx1_export,
                1,
                &export_ch,
                "#ch",
                &root_ts,
            );
            assert_eq!(th_doc.workspace, target_ws_id.as_str());
            assert_eq!(
                th_doc.messages[0].files[0].transcript.as_deref(),
                Some("Spoken note in ws1 thread")
            );

            // Jump from workspace 1 back to workspace 0 via open_message_in_workspace.
            app.open_message_in_workspace(&ws0_id, ws0_ch.clone(), ws0_ts, window, cx);
            assert_eq!(app.workspace, 0);
            assert_eq!(app.channel, ws0_ch);
        });
    });
    cx.run_until_parked();

    // Verify workspace-1 search actually produced workspace-1 hits in CurrentWorkspace scope,
    // and switching to AllWorkspaces returns hits attributed to their respective workspaces.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let ws1_id = app.fixtures.workspaces[1].id.clone();
            app.switch_workspace(1, window, cx);
            let ws1_word = app
                .fixtures
                .messages
                .values()
                .flatten()
                .flat_map(|m| m.text.split_whitespace())
                .find(|w| w.len() >= 5 && w.chars().all(|c| c.is_ascii_alphabetic()))
                .unwrap()
                .to_owned();
            app.run_message_search(ws1_word, cx);
            app.results.update(cx, |v, cx| {
                v.set_scope(crate::search::SearchScope::CurrentWorkspace, cx);
            });
            assert_eq!(app.results.read(cx).active_workspace_for_test(), ws1_id);
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        app.read_with(cx, |app, cx| {
            let ws1_id = &app.fixtures.workspaces[1].id;
            let hits = app.results.read(cx).local_hits_for_test();
            assert!(
                !hits.is_empty(),
                "CurrentWorkspace search on workspace index 1 must return hits"
            );
            assert!(hits.iter().all(|h| h.workspace == *ws1_id));
        });
    });

    // Now test AllWorkspaces scope from workspace 1 with a word present in workspace 0.
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let ws0_fx = fixtures::workspace(0);
            let ws0_word = ws0_fx
                .messages
                .values()
                .flatten()
                .flat_map(|m| m.text.split_whitespace())
                .find(|w| w.len() >= 6 && w.chars().all(|c| c.is_ascii_alphabetic()))
                .expect("workspace 0 searchable word")
                .to_owned();
            app.run_message_search(ws0_word, cx);
            app.results.update(cx, |v, cx| {
                v.set_scope(crate::search::SearchScope::AllWorkspaces, cx);
            });
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        app.read_with(cx, |app, cx| {
            let ws0_id = &app.fixtures.workspaces[0].id;
            let hits = app.results.read(cx).local_hits_for_test();
            assert!(
                hits.iter().any(|h| h.workspace == *ws0_id),
                "AllWorkspaces search from workspace 1 must find workspace 0 hits"
            );
        });
    });
    drop(root);
}

#[test]
fn duplicate_slash_candidates_broadcast_guards_locked_profile_and_settings_defaults() {
    use slackless_core::api::AppCommand;
    use slackless_core::types::{Conversation, ConversationProperties, User};

    // 1. Duplicate slash command disambiguation candidates (F-COLLAB-686)
    let commands = vec![
        AppCommand {
            name: "deploy".to_owned(),
            description: "Deploy via CI".to_owned(),
            usage: "[env]".to_owned(),
            app_name: Some("CI Bot".to_owned()),
            app_id: Some("A1".to_owned()),
        },
        AppCommand {
            name: "deploy".to_owned(),
            description: "Deploy via Release Manager".to_owned(),
            usage: "[service]".to_owned(),
            app_name: Some("ReleaseBot".to_owned()),
            app_id: Some("A2".to_owned()),
        },
        AppCommand {
            name: "shrug".to_owned(),
            description: "Custom shrug".to_owned(),
            usage: String::new(),
            app_name: Some("FunApp".to_owned()),
            app_id: Some("A3".to_owned()),
        },
    ];
    let deploy_inv = crate::slash::parse("/deploy prod").expect("parse /deploy");
    let deploy_candidates = matching_slash_candidates(&deploy_inv, &commands);
    assert_eq!(deploy_candidates.len(), 2);
    assert_eq!(deploy_candidates[0].app_id.as_deref(), Some("A1"));
    assert_eq!(deploy_candidates[1].app_id.as_deref(), Some("A2"));

    // Built-in command shadowed by an app command also surfaces both candidates.
    let shrug_inv = crate::slash::parse("/shrug hi").expect("parse /shrug");
    let shrug_candidates = matching_slash_candidates(&shrug_inv, &commands);
    assert_eq!(shrug_candidates.len(), 2);
    assert!(shrug_candidates[0].is_builtin);
    assert_eq!(shrug_candidates[1].app_id.as_deref(), Some("A3"));

    // 2. Large-channel & restricted broadcast mention guardrails (F-COLLAB-1248..1249)
    let mut small_ch = Conversation {
        id: ChannelId::from("C1"),
        name: Some("team".to_owned()),
        is_channel: true,
        ..Default::default()
    };
    assert!(evaluate_broadcast_guard(&BroadcastRange::Channel, &small_ch, 4, None).is_none());
    assert_eq!(broadcast_autocomplete_hint(&small_ch, 4), None);

    let big_guard = evaluate_broadcast_guard(&BroadcastRange::Channel, &small_ch, 42, None)
        .expect("large channel triggers confirmation");
    assert!(big_guard.body.contains("42 people"));
    assert_eq!(big_guard.member_count, 42);
    assert!(!big_guard.is_restricted);
    assert_eq!(
        broadcast_autocomplete_hint(&small_ch, 42).as_deref(),
        Some("42 members")
    );

    // Enterprise large channel (>= 10,000 members) warns non-admins as restricted.
    let member = User {
        is_admin: false,
        ..Default::default()
    };
    let ent_guard =
        evaluate_broadcast_guard(&BroadcastRange::Here, &small_ch, 10_500, Some(&member))
            .expect("enterprise channel triggers warning");
    assert!(ent_guard.is_restricted);
    assert!(ent_guard.restriction_note.is_some());
    assert!(mention_permission_summary(&small_ch, 10_500).contains("10500"));

    // Explicit channel restriction via properties.at_channel_restricted
    let mut props = ConversationProperties::default();
    props
        .extra
        .insert("at_channel_restricted".to_owned(), serde_json::json!(true));
    small_ch.properties = Some(props);
    let restricted_guard =
        evaluate_broadcast_guard(&BroadcastRange::Channel, &small_ch, 3, Some(&member))
            .expect("restricted channel always warns");
    assert!(restricted_guard.is_restricted);
    assert_eq!(
        broadcast_autocomplete_hint(&small_ch, 3).as_deref(),
        Some("Restricted")
    );
    assert!(
        mention_permission_summary(&small_ch, 3).contains("restricted to Channel Managers"),
        "unexpected permission summary: {}",
        mention_permission_summary(&small_ch, 3)
    );

    // 3. Locked profile fields detection (F-SHELL-005)
    let mut scim_user = User::default();
    scim_user
        .extra
        .insert("is_scim_managed".to_owned(), serde_json::json!(true));
    let locks = detect_locked_profile_fields(
        Some(&scim_user),
        &[(
            "Department".to_owned(),
            "Managed by workspace admin".to_owned(),
        )],
    );
    assert!(locks.contains_key("display_name"));
    assert!(locks.contains_key("title"));
    assert!(locks.contains_key("phone"));
    assert!(locks.contains_key("department"));

    // 4. Settings defaults & serde round-trip (F-SHELL-010, F-SHELL-164)
    let defaults = crate::preferences::Settings::default();
    assert!(!defaults.beta_updates);
    assert!(defaults.show_pronouns);
    assert_eq!(
        defaults.update_channel(),
        crate::updater::UpdateChannel::Stable
    );

    // Old JSON lacking new keys deserializes with safe defaults (show_pronouns=true, beta_updates=false)
    let migrated: crate::preferences::Settings =
        serde_json::from_str("{}").expect("empty JSON migrates");
    assert!(!migrated.beta_updates);
    assert!(migrated.show_pronouns);

    let mut beta_settings = defaults;
    beta_settings.beta_updates = true;
    beta_settings.show_pronouns = false;
    assert_eq!(
        beta_settings.update_channel(),
        crate::updater::UpdateChannel::Beta
    );
    let json = serde_json::to_string(&beta_settings).expect("serialize settings");
    let roundtrip: crate::preferences::Settings =
        serde_json::from_str(&json).expect("deserialize settings");
    assert!(roundtrip.beta_updates);
    assert!(!roundtrip.show_pronouns);
}

#[gpui::test]
fn parity_finish_gui_slash_disambiguation_profile_general_rename_and_beta_prefs(
    cx: &mut gpui::TestAppContext,
) {
    let mut fx = fixtures::load();
    fx.app_commands.push(slackless_core::api::AppCommand {
        name: "deploy".to_owned(),
        description: "Deploy via CI".to_owned(),
        usage: "[env]".to_owned(),
        app_name: Some("CI Bot".to_owned()),
        app_id: Some("A1".to_owned()),
    });
    fx.app_commands.push(slackless_core::api::AppCommand {
        name: "deploy".to_owned(),
        description: "Deploy via Release".to_owned(),
        usage: "[env]".to_owned(),
        app_name: Some("ReleaseBot".to_owned()),
        app_id: Some("A2".to_owned()),
    });
    let (root, app, cx) = mount_test_app(fx, cx);

    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            // 1. Slash command CommandResponse (in_channel & ephemeral) + CommandFailed (F-COLLAB-684..685)
            let channel = app.channel.clone();
            let before = app.messages.read(cx).message_count_for_test(cx);
            app.on_core_event(
                &Event::CommandResponse {
                    channel: channel.clone(),
                    command: "/deploy".to_owned(),
                    response_type: Some("in_channel".to_owned()),
                    text: "Build #42 deployed".to_owned(),
                    thread_ts: None,
                },
                window,
                cx,
            );
            assert_eq!(app.messages.read(cx).message_count_for_test(cx), before + 1);
            assert_eq!(
                app.messages
                    .read(cx)
                    .last_message_text_for_test(cx)
                    .as_deref(),
                Some("Build #42 deployed")
            );

            app.on_core_event(
                &Event::CommandFailed {
                    command: "/deploy".to_owned(),
                    error: "timeout".to_owned(),
                },
                window,
                cx,
            );
            assert_eq!(app.messages.read(cx).message_count_for_test(cx), before + 2);
            let last_err = app
                .messages
                .read(cx)
                .last_message_text_for_test(cx)
                .expect("ephemeral timeout error rendered");
            assert!(
                last_err.contains("did not respond in time"),
                "expected timeout feedback, got: {last_err}"
            );

            // 2. Duplicate slash command disambiguation opens modal dialog (F-COLLAB-686)
            let inv = crate::slash::parse("/deploy prod").expect("parse /deploy");
            app.run_slash(&inv, None, window, cx);
            assert!(
                window.has_active_dialog(cx),
                "duplicate slash command must open disambiguation dialog"
            );
            window.close_dialog(cx);

            // Selecting a duplicate app with app_id=None + distinct app_name must NOT loop back
            // into the disambiguation dialog.
            let chosen_by_name = inv.with_app(None, Some("ReleaseBot".to_owned()));
            app.run_slash(&chosen_by_name, None, window, cx);
            assert!(
                !window.has_active_dialog(cx),
                "choosing an app by app_name with app_id=None must not re-open disambiguation"
            );

            // 3. ProfilePanel locked fields, automatic status source, and pronoun toggle (F-SHELL-005/010/020..021)
            let self_id = app.fixtures.self_user.clone();
            app.profile.update(cx, |p, cx| {
                p.set_user_profile(
                    self_id.clone(),
                    Some("they/them".to_owned()),
                    vec![("Department".to_owned(), "Engineering".to_owned())],
                    vec![(
                        "Department".to_owned(),
                        "Managed by SCIM / identity provider".to_owned(),
                    )],
                    cx,
                );
                p.set_show_pronouns(false, cx);
                p.open(self_id.clone(), false, cx);
            });
            assert_eq!(
                app.profile.read(cx).loaded_locked_fields(&self_id),
                vec![(
                    "Department".to_owned(),
                    "Managed by SCIM / identity provider".to_owned()
                )]
            );
            app.right_pane = RightPane::Profile { over_thread: false };

            // 4. Open #general channel details so F-COLLAB-1236 rename path renders cleanly
            if let Some(gen_ch) = app.fixtures.channels.iter().find(|c| c.is_general) {
                let gen_id = gen_ch.id.clone();
                let fx_arc = Arc::clone(&app.fixtures);
                app.details.update(cx, |d, cx| {
                    d.open(
                        fx_arc,
                        gen_id,
                        Vec::new(),
                        false,
                        crate::channel_details::DetailsTab::About,
                        cx,
                    );
                });
            }

            // 5. Preferences -> Advanced renders Beta updates toggle + prerelease badge (F-SHELL-164)
            let beta_info = crate::updater::ReleaseInfo {
                version: "2026.12.01-beta.1".to_owned(),
                html_url: "https://github.com/ankarhem/slackless/releases/tag/v2026.12.01-beta.1"
                    .to_owned(),
                asset_name: "Slackless-2026.12.01-beta.1-macos-arm64.app.tar.gz".to_owned(),
                download_url: "https://example.com/beta.tar.gz".to_owned(),
                sha256_url: None,
                notes: vec!["Beta preview build".to_owned()],
                is_prerelease: true,
            };
            app.preferences.update(cx, |prefs, cx| {
                let mut s = prefs.settings().clone();
                s.beta_updates = true;
                prefs.load_settings(s, window, cx);
                prefs.set_update_state(
                    crate::updater::UpdateState::Available(beta_info.clone()),
                    Some(beta_info),
                    cx,
                );
                prefs.show(Section::Advanced, cx);
            });
            assert!(app.preferences.read(cx).settings().beta_updates);
        });
        let _ = window.draw(cx);
    });
    drop(root);
}

#[gpui::test]
fn qa_wave1_modal_stale_submit_workspace_switch_and_viewer_shortcut_guards(
    cx: &mut gpui::TestAppContext,
) {
    use gpui_kit::base::Root;
    use slackless_core::api::ViewSubmitResponse;
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
            // 1. Open a modal on workspace 0, then deliver stale submit errors for a closed view_id.
            let view = serde_json::json!({
                "id": "V_ACTIVE",
                "title": { "type": "plain_text", "text": "Active Modal" },
                "submit": { "type": "plain_text", "text": "Submit" },
                "blocks": [{
                    "type": "input",
                    "block_id": "b1",
                    "label": { "type": "plain_text", "text": "Name" },
                    "element": { "type": "plain_text_input", "action_id": "a1" }
                }]
            });
            app.modal_stack
                .update(cx, |stack, cx| stack.open_view(view, window, cx));
            assert_eq!(app.modal_stack.read(cx).depth(), 1);

            let mut stale_errs = HashMap::new();
            stale_errs.insert("b1".to_owned(), "Stale field error".to_owned());
            app.modal_stack.update(cx, |stack, cx| {
                stack.apply_submit_result(
                    "V_CLOSED",
                    Ok(ViewSubmitResponse::Errors(stale_errs)),
                    window,
                    cx,
                );
                stack.apply_submit_result(
                    "V_CLOSED",
                    Err("Stale transport failure".to_owned()),
                    window,
                    cx,
                );
            });
            let active_kit = app.modal_stack.read(cx).top_kit().expect("active kit");
            assert_eq!(
                active_kit.read(cx).block_error("b1"),
                None,
                "stale view_id validation errors must not attach to active modal"
            );
            assert_eq!(
                app.modal_stack.read(cx).frames[0].submit_error,
                None,
                "stale view_id submit error must not attach to active modal"
            );

            // 2. Switching workspaces must dismiss open modals from the outgoing workspace.
            app.switch_workspace(1, window, cx);
            assert_eq!(app.workspace, 1);
            assert_eq!(
                app.modal_stack.read(cx).depth(),
                0,
                "switch_workspace must dismiss open modal stack"
            );

            // 3. While ImageViewer is open, single-key transcript shortcuts must not fire.
            app.messages
                .update(cx, |list, cx| list.step_focus(true, cx));
            let items = Rc::new(vec![crate::image_viewer::ViewerImage {
                source: gpui::ImageSource::from(SharedString::from("https://example.com/a.png")),
                name: "a.png".into(),
                user: None,
                taken_at: None,
                download: None,
                video: None,
                pdf: None,
            }]);
            app.open_image_viewer(app.channel.clone(), items, 0, window, cx);
            assert!(app.image_viewer.is_some());
            app.open_remind_menu(window, cx);
            assert!(
                app.remind_menu.is_none(),
                "remind menu shortcut must be inert while ImageViewer is open"
            );
            let before_thread = app.right_pane.is_thread();
            app.focus_message(MessageAction::ReplyInThread, window, cx);
            assert_eq!(
                app.right_pane.is_thread(),
                before_thread,
                "focused-message shortcuts must be inert while ImageViewer is open"
            );
            app.close_image_viewer(window, cx);
        });
    });
    cx.run_until_parked();
    drop(root);
}

#[gpui::test]
fn app_outbox_events_inline_thread_sync_and_catchup_navigation(cx: &mut gpui::TestAppContext) {
    use gpui_kit::base::Root;
    use slackless_core::api::MessageContent;
    use slackless_core::catchup::CatchUpWindow;
    use slackless_core::outbox::OutboxPayload;
    use slackless_core::store::{OutboxItem, OutboxKind, OutboxStatus};
    use std::cell::RefCell;

    cx.update(|cx| {
        cx.set_global(crate::zoom::Zoom::default());
        gpui_kit::init(cx);
        theme::init(cx);
    });
    let mounted = Rc::new(RefCell::new(None));
    let slot = mounted.clone();
    let (root, cx) = cx.add_window_view(move |window, cx| {
        let fx = Arc::new(fixtures::workspace(0));
        let app = App::new(fx, CoreBridge::offline(), window, cx);
        *slot.borrow_mut() = Some(app.clone());
        Root::new(app, window, cx)
    });
    let app = mounted.borrow_mut().take().unwrap();
    cx.run_until_parked();

    // 1. OutboxUpdated populates channel + thread outbox state and renders cleanly.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let ws_id = app.fixtures.workspaces[0].id.clone();
            let ch = app.channel.clone();
            let item_ch = OutboxItem {
                id: 101,
                workspace_id: ws_id.clone(),
                channel_id: ch.clone(),
                thread_ts: None,
                kind: OutboxKind::Message,
                payload_json: OutboxPayload::Message {
                    content: MessageContent::plain("Queued while on a flight"),
                    client_msg_id: None,
                }
                .encode(),
                status: OutboxStatus::Pending,
                attempts: 0,
                last_error: None,
                created_at: 1_750_000_000,
                updated_at: 1_750_000_000,
            };
            let item_reply = OutboxItem {
                id: 102,
                workspace_id: ws_id,
                channel_id: ch.clone(),
                thread_ts: Some(Ts::from("1700000001.000100")),
                kind: OutboxKind::Reply,
                payload_json: OutboxPayload::Reply {
                    content: MessageContent::plain("Failed thread reply"),
                    broadcast: false,
                    client_msg_id: None,
                }
                .encode(),
                status: OutboxStatus::Failed,
                attempts: 1,
                last_error: Some("503 service unavailable".into()),
                created_at: 1_750_000_001,
                updated_at: 1_750_000_001,
            };
            app.on_core_event(&Event::OutboxUpdated(vec![item_ch, item_reply]), window, cx);
            assert_eq!(app.ws.outbox.len(), 2);
            app.open_thread(ch, Ts::from("1700000001.000100"), cx);
        });
    });
    cx.run_until_parked();

    // 2. Open Catch-Up view, switch window & scope, and jump back to a channel.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_catchup(window, cx);
            assert_eq!(app.page, MainPage::CatchUp);
            assert_eq!(app.current_place(cx), NavPlace::CatchUp);
            app.catchup.update(cx, |v, cx| {
                v.select_window(CatchUpWindow::Last7d, cx);
                v.select_scope(true, cx);
            });
        });
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            assert_eq!(app.catchup.read(cx).window(), CatchUpWindow::Last7d);
            assert!(app.catchup.read(cx).all_workspaces());
            let ws1_id = app.fixtures.workspaces[1].id.clone();
            let ws1_ch = fixtures::workspace(1).channels[0].id.clone();
            app.open_catchup_channel(&ws1_id, ws1_ch.clone(), None, window, cx);
            assert_eq!(app.workspace, 1);
            assert_eq!(app.page, MainPage::Channel);
            assert_eq!(app.channel, ws1_ch);
        });
    });
    cx.run_until_parked();
    drop(root);
}

#[gpui::test]
fn smart_notification_rules_and_daily_catchup_reminder_in_app(cx: &mut gpui::TestAppContext) {
    use slackless_core::rules::{NotificationRule, RuleAction};
    use slackless_core::types::time::days_from_civil;

    let mut fx = fixtures::load();
    let me = fx.self_user.clone();
    let peer = fx
        .users
        .iter()
        .find(|u| u.id != me)
        .expect("peer user")
        .id
        .clone();
    let eng = ChannelId::from("C0ENG");
    let general = ChannelId::from("C0GENERAL");
    // Mute #general, set #engineering to Mentions-only.
    fx.notify_levels.insert(general.clone(), NotifyLevel::Muted);
    fx.notify_levels.insert(eng.clone(), NotifyLevel::Mentions);

    let (root, app, cx) = mount_test_app(fx, cx);
    let plain_msg = |user: &UserId, text: &str| Message {
        ts: Ts::from("1760000100.000100"),
        user: Some(user.clone()),
        text: text.to_owned(),
        ..Default::default()
    };

    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            // Switch off #general and #engineering so neither is on-screen.
            app.select(ChannelId::from("C0DESIGN"), cx);
            app.preferences.update(cx, |prefs, cx| {
                let mut s = prefs.settings().clone();
                s.desktop_notifications = true;
                s.schedule_enabled = false;
                s.notify_level = 1; // Mentions only
                s.smart_notification_rules = vec![
                    NotificationRule {
                        id: "r-suppress-bot".into(),
                        name: "Suppress build noise".into(),
                        enabled: true,
                        action: RuleAction::Suppress,
                        in_channels: vec!["#engineering".into()],
                        keywords: vec!["build-noise".into()],
                        ..Default::default()
                    },
                    NotificationRule {
                        id: "r-breakthrough".into(),
                        name: "SEV-1 page".into(),
                        enabled: true,
                        action: RuleAction::BreakThroughMute,
                        keywords: vec!["sev-1".into()],
                        ..Default::default()
                    },
                    NotificationRule {
                        id: "r-priority".into(),
                        name: "Deploy highlight".into(),
                        enabled: true,
                        action: RuleAction::PriorityHighlight,
                        in_channels: vec!["#engineering".into()],
                        keywords: vec!["deploy-window".into()],
                        ..Default::default()
                    },
                ];
                prefs.load_settings(s, window, cx);
            });

            // 1. Muted #general normally silent, but SEV-1 breaks through mute AND focus mode!
            assert!(!app.should_notify(&general, &plain_msg(&peer, "normal chatter"), window, cx));
            assert!(app.should_notify(
                &general,
                &plain_msg(&peer, "SEV-1 prod outage"),
                window,
                cx
            ));
            app.self_state
                .start_focus_with_status(FOCUS_UNTIL_OFF, false);
            assert!(app.should_notify(
                &general,
                &plain_msg(&peer, "SEV-1 while focused"),
                window,
                cx
            ));
            app.self_state.end_focus();

            // 2. Suppress silences matching messages even when mentioned!
            let mentioned_noise = plain_msg(&peer, &format!("<@{me}> build-noise nightly green"));
            assert!(!app.should_notify(&eng, &mentioned_noise, window, cx));

            // 3. Conflict resolution: BreakThroughMute beats Suppress on the same message.
            let sev1_noise = plain_msg(&peer, "build-noise SEV-1 rollback");
            assert!(app.should_notify(&eng, &sev1_noise, window, cx));

            // 4. PriorityHighlight elevates non-mention message in Mentions-only channel,
            //    while still respecting channel mute.
            assert!(app.should_notify(
                &eng,
                &plain_msg(&peer, "deploy-window starts now"),
                window,
                cx
            ));
            // Suppress beats PriorityHighlight when both match.
            assert!(!app.should_notify(
                &eng,
                &plain_msg(&peer, "build-noise deploy-window"),
                window,
                cx
            ));

            // 5. Daily Catch-Up reminder fires at most once per calendar day.
            app.preferences.update(cx, |prefs, cx| {
                let mut s = prefs.settings().clone();
                s.catchup_reminder_enabled = true;
                s.catchup_reminder_time = "09:00".to_owned();
                s.catchup_reminder_last_fired_day = None;
                prefs.load_settings(s, window, cx);
            });
            let day_26_utc = days_from_civil(2026, 11, 26) * 86_400;
            // 08:30 UTC (tz 0) -> too early, does not fire.
            app.check_daily_catchup_reminder_at(day_26_utc + 8 * 3_600 + 30 * 60, 0, window, cx);
            assert_eq!(
                app.preferences
                    .read(cx)
                    .settings()
                    .catchup_reminder_last_fired_day,
                None
            );
            // 09:05 UTC -> fires and records 2026-11-26.
            app.check_daily_catchup_reminder_at(day_26_utc + 9 * 3_600 + 5 * 60, 0, window, cx);
            assert_eq!(
                app.preferences
                    .read(cx)
                    .settings()
                    .catchup_reminder_last_fired_day
                    .as_deref(),
                Some("2026-11-26")
            );
            assert!(
                app.recent_notified_tags
                    .iter()
                    .any(|(tag, _)| tag == "catchup:2026-11-26")
            );
            // Calling again later the same day does not re-fire.
            let count_before = app.recent_notified_tags.len();
            app.check_daily_catchup_reminder_at(day_26_utc + 15 * 3_600, 0, window, cx);
            assert_eq!(app.recent_notified_tags.len(), count_before);
        });
    });
    cx.run_until_parked();
    drop(root);
}

#[gpui::test]
fn deep_link_hardening_multi_workspace_threads_files_and_unsigned_workspace(
    cx: &mut gpui::TestAppContext,
) {
    let mut fx = fixtures::load();
    let ws0_id = fx.workspaces[0].id.clone();
    let ws1_id = fx.workspaces[1].id.clone();
    let ws1_domain = fx.workspaces[1].domain.clone();
    let ws1_fx = fixtures::workspace(1);
    let ws1_ch = ws1_fx.channels[0].id.clone();

    let eng = ChannelId::from("C0ENG");
    let eng_thread_ts = fx
        .messages
        .get(&eng)
        .and_then(|msgs| msgs.iter().find(|m| m.reply_count.unwrap_or(0) > 0))
        .map(|m| m.ts.clone())
        .expect("C0ENG has thread root");
    let reply_ts = fx
        .threads
        .get(&(eng.clone(), eng_thread_ts.clone()))
        .and_then(|r| r.first())
        .map(|m| m.ts.clone())
        .unwrap_or_else(|| Ts::from("1760000000.065432"));

    // Attach a known file ID to a message in #engineering so slack://file resolves directly.
    if let Some(msgs) = fx.messages.get_mut(&eng)
        && let Some(first) = msgs.first_mut()
    {
        first.files.push(slackless_core::types::File {
            id: slackless_core::types::FileId::from("F0DEEPFILE"),
            name: Some("spec.pdf".into()),
            title: Some("Spec".into()),
            ..Default::default()
        });
    }

    let (root, app, cx) = mount_test_app(fx, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            // 1. slack://channel with message & thread_ts opens exact thread in target workspace.
            assert!(app.handle_deep_link(
                &format!(
                    "slack://channel?team={ws0_id}&id=C0ENG&message={reply_ts}&thread_ts={eng_thread_ts}"
                ),
                window,
                cx
            ));
            assert_eq!(app.workspace, 0);
            assert_eq!(app.channel, eng);
            assert!(app.right_pane.is_thread());
            assert_eq!(
                app.thread.read(cx).open_thread(),
                Some((eng.clone(), eng_thread_ts.clone()))
            );
            app.close_thread(cx);

            // 2. slack://file jumps to the channel containing the file when present locally.
            app.select(ChannelId::from("C0DESIGN"), cx);
            assert!(app.handle_deep_link(
                &format!("slack://file?team={ws0_id}&id=F0DEEPFILE"),
                window,
                cx
            ));
            assert_eq!(app.channel, eng);

            // 3. slack://file for an uncached file opens the Files page cleanly.
            assert!(app.handle_deep_link(
                &format!("slack://file?team={ws0_id}&id=F_UNKNOWN_REMOTE"),
                window,
                cx
            ));
            assert_eq!(app.page, MainPage::Files);

            // 4. Multi-workspace routing via slack://open switches to workspace 1.
            assert!(app.handle_deep_link(
                &format!("slack://open?team={ws1_id}&id={ws1_ch}"),
                window,
                cx
            ));
            assert_eq!(app.workspace, 1);
            assert_eq!(app.channel, ws1_ch);

            // 5. Switch back to workspace 0, then jump to workspace 1 via https://<ws1_domain>.slack.com/archives/...
            app.switch_workspace(0, window, cx);
            assert_eq!(app.workspace, 0);
            if !ws1_domain.is_empty() {
                assert!(app.handle_deep_link(
                    &format!("https://{ws1_domain}.slack.com/archives/{ws1_ch}/p1700000001234567"),
                    window,
                    cx
                ));
                assert_eq!(app.workspace, 1);
                assert_eq!(app.channel, ws1_ch);
            }

            // 6. Unsigned-in target workspace does NOT switch workspace, surfaces toast, and returns true.
            let prev_ws = app.workspace;
            let prev_ch = app.channel.clone();
            assert!(app.handle_deep_link(
                "slack://channel?team=T_NOT_SIGNED_IN&id=C999999",
                window,
                cx
            ));
            assert_eq!(app.workspace, prev_ws);
            assert_eq!(app.channel, prev_ch);

            assert!(app.handle_deep_link(
                "https://unrecognized-org.slack.com/archives/C999999/p1700000001234567",
                window,
                cx
            ));
            assert_eq!(app.workspace, prev_ws);
            assert_eq!(app.channel, prev_ch);
        });
    });
    cx.run_until_parked();
    drop(root);
}
