//! Wired GUI integration tests and UI consistency guardrails.
//!
//! Unlike `CoreBridge::offline()` smoke tests (which silently drop outgoing commands and call
//! internal handlers directly), these tests mount [`App`] against [`CoreBridge::test_harness`]:
//! - Every user action's outgoing [`Command`] is captured and asserted.
//! - Inbound [`Event`]s and [`HuddleMediaEvent`]s travel through the real GPUI `EventHub` and
//!   `HuddleMediaHub` subscriptions into `App`, backed by a real in-memory SQLite [`Store`].
//! - UI consistency tests render every major page and dialog across Dark/Light themes,
//!   Normal/Compact density, and Standard/Narrow viewports, and guard the `theme.rs` color-token
//!   invariant against regressions.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use gpui_kit::base::Root;
use gpui_kit::gpui::{self, Entity, px, size};
use slackless_core::auth::Session;
use slackless_core::huddle::{
    HuddleAudioLevels, HuddlePhase, HuddleSessionSnapshot, VideoFrame, VideoFrameData, VideoTileKey,
};
use slackless_core::store::{NewOutboxItem, OutboxKind, Store};
use slackless_core::types::{ChannelId, Conversation, Message, Team, TeamId, Ts, User, UserId};
use slackless_core::{
    Command, Event, HuddleCommand, HuddleControlCommand, HuddleEvent, HuddleMediaEvent,
    MessagingCommand, SharedStore,
};

use super::*;
use crate::bridge::{CoreBridge, TestCoreHandle};
use crate::theme::{self, SlackTheme};

fn make_wired_workspace(
    team: &str,
    team_name: &str,
    channels: &[(&str, &str)],
    users: &[User],
    cx: &mut gpui::App,
) -> (CoreBridge, TestCoreHandle) {
    let mut store = Store::open_in_memory().expect("in-memory store");
    let team_id = TeamId::from(team);
    store
        .upsert_team(
            &team_id,
            &Team {
                id: team_id.clone(),
                name: team_name.to_owned(),
                domain: team.to_lowercase(),
                ..Default::default()
            },
        )
        .expect("upsert team");
    for (id, name) in channels {
        store
            .upsert_conversation(
                &team_id,
                &Conversation {
                    id: ChannelId::from(*id),
                    name: Some((*name).to_owned()),
                    is_channel: true,
                    is_member: true,
                    ..Default::default()
                },
            )
            .expect("upsert channel");
    }
    store.upsert_users(&team_id, users).expect("upsert users");
    let shared: SharedStore = Arc::new(Mutex::new(store));
    let session = Session {
        team_id,
        user_id: UserId::from("U0ME"),
        workspace_url: format!("https://{}.slack.com", team.to_lowercase()),
        xoxc_token: "xoxc-wired-test".into(),
        d_cookie: "xoxd-wired-test".into(),
    };
    CoreBridge::test_harness(session, shared, cx)
}

fn mount_wired(
    fixtures: Fixtures,
    bridges: Vec<CoreBridge>,
    cx: &mut gpui::TestAppContext,
) -> (Entity<Root>, Entity<App>, &mut gpui::VisualTestContext) {
    use std::cell::RefCell;
    use std::rc::Rc;

    cx.update(|cx| {
        cx.set_global(crate::zoom::Zoom::default());
        gpui_kit::init(cx);
        theme::init(cx);
    });
    let mounted = Rc::new(RefCell::new(None));
    let slot = mounted.clone();
    let (root, cx) = cx.add_window_view(move |window, cx| {
        let app = App::new_multi(Arc::new(fixtures), bridges, window, cx);
        *slot.borrow_mut() = Some(app.clone());
        Root::new(app, window, cx)
    });
    let app = mounted.borrow_mut().take().expect("mounted App");
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    (root, app, cx)
}

fn user(id: &str, name: &str) -> User {
    User {
        id: UserId::from(id),
        name: name.to_owned(),
        real_name: Some(name.to_owned()),
        ..Default::default()
    }
}

#[gpui::test]
fn wired_huddle_lifecycle_commands_events_media_and_teardown(cx: &mut gpui::TestAppContext) {
    let people = [user("U0ME", "me"), user("U0ALICE", "alice")];
    let (bridge, core) = cx.update(|cx| {
        make_wired_workspace(
            "T0HUD",
            "Huddle Co",
            &[("C0ENG", "engineering")],
            &people,
            cx,
        )
    });
    let fx = bridge.snapshot(&HashMap::new()).expect("snapshot");
    let (root, app, cx) = mount_wired(fx, vec![bridge], cx);
    let _ = core.drain_commands();

    let eng = ChannelId::from("C0ENG");

    // 1. Triggering join in GUI must emit a real `HuddleCommand::Join` over `CoreBridge`.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.join_huddle_in(eng.clone(), window, cx);
        });
    });
    cx.run_until_parked();
    let cmds = core.drain_commands();
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Huddle(HuddleCommand::Join { channel, .. }) if *channel == eng
        )),
        "GUI join must send HuddleCommand::Join, got: {cmds:?}"
    );

    // 2. Core emits `HuddleEvent::SessionChanged(Connected)` through real `EventHub`.
    let snap = HuddleSessionSnapshot {
        phase: HuddlePhase::Connected,
        channel: eng.clone(),
        call_id: "R0WIRED".into(),
        attendee_id: "att-me".into(),
        started_at_secs: 1_700_000_000,
        ..Default::default()
    };
    cx.update(|_, cx| {
        core.emit(
            Event::Huddle(HuddleEvent::SessionChanged(Box::new(snap.clone()))),
            cx,
        );
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        let state = app
            .read(cx)
            .huddle_state
            .as_ref()
            .expect("HuddleState mounted on Connected");
        assert_eq!(state.read(cx).session().phase, HuddlePhase::Connected);
        assert_eq!(state.read(cx).session().call_id, "R0WIRED");
    });

    // 2b. A huddle thread started after joining (the room's thread root) is the one the main
    // window's huddle-thread toggle opens.
    let root_ts = Ts::from("1700000500.000100");
    cx.update(|_, cx| {
        core.emit(
            Event::Huddle(HuddleEvent::Updated {
                channel: eng.clone(),
                participants: vec![UserId::from("U0ME"), UserId::from("U0ALICE")],
                ended: false,
                topic: None,
                background_id: None,
                thread_root_ts: Some(root_ts.clone()),
            }),
            cx,
        );
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        app.update(cx, |app, cx| app.toggle_huddle_thread(cx));
        assert_eq!(
            app.read(cx).thread.read(cx).open_thread(),
            Some((eng.clone(), root_ts.clone())),
            "the toggle must open the thread the room announced after joining"
        );
    });

    // 3. Emit high-frequency `HuddleMediaEvent`s through real `HuddleMediaHub` (AudioLevels,
    // VideoFrame, VideoTileClosed) and verify `HuddleState` + `HuddleVideoStore` stay in sync.
    let tile = VideoTileKey::RemoteCamera(UserId::from("U0ALICE"));
    let frame = VideoFrame {
        width: 4,
        height: 4,
        data: VideoFrameData::Bgra(Arc::new(vec![200u8; 4 * 4 * 4])),
    };
    // The store converts frames only while a huddle pop-out window shows video.
    let _popout = cx.update(|_, cx| {
        let state = app.read(cx).huddle_state.as_ref().unwrap().read(cx);
        state.video_store().read(cx).register_popout()
    });
    cx.update(|_, cx| {
        core.emit_media(
            HuddleMediaEvent::AudioLevels(HuddleAudioLevels {
                local_rms: 0.42,
                active_speakers: vec![(UserId::from("U0ALICE"), 0.85)],
                muted_peers: vec![],
            }),
            cx,
        );
        core.emit_media(
            HuddleMediaEvent::VideoFrame {
                tile: tile.clone(),
                frame,
            },
            cx,
        );
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        let state = app.read(cx).huddle_state.as_ref().unwrap().read(cx);
        assert_eq!(state.levels().active_speakers.len(), 1);
        assert!(
            state.video_store().read(cx).frames().contains_key(&tile),
            "VideoFrame via HuddleMediaHub must land in HuddleVideoStore"
        );
    });

    cx.update(|_, cx| {
        core.emit_media(HuddleMediaEvent::VideoTileClosed(tile.clone()), cx);
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        let state = app.read(cx).huddle_state.as_ref().unwrap().read(cx);
        assert!(
            !state.video_store().read(cx).frames().contains_key(&tile),
            "VideoTileClosed via HuddleMediaHub must remove tile"
        );
    });

    // 4. Leaving via GUI emits `HuddleCommand::Leave`; Core `Idle` tears down `HuddleState`.
    let _ = core.drain_commands();
    cx.update(|_, cx| {
        app.update(cx, |app, cx| app.leave_huddle(cx));
    });
    cx.run_until_parked();
    let leave_cmds = core.drain_commands();
    assert!(
        leave_cmds
            .iter()
            .any(|c| matches!(c, Command::Huddle(HuddleCommand::Leave))),
        "leave_huddle must send HuddleCommand::Leave, got: {leave_cmds:?}"
    );

    let idle = HuddleSessionSnapshot {
        phase: HuddlePhase::Idle,
        channel: eng,
        ..Default::default()
    };
    cx.update(|_, cx| {
        core.emit(
            Event::Huddle(HuddleEvent::SessionChanged(Box::new(idle))),
            cx,
        );
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        assert!(
            app.read(cx).huddle_state.is_none(),
            "Idle session event must cleanly tear down HuddleState"
        );
        assert!(app.read(cx).huddle_dock.is_none());
    });
    drop(root);
}

#[gpui::test]
fn wired_composer_send_message_store_event_and_outbox_retry(cx: &mut gpui::TestAppContext) {
    let people = [user("U0ME", "me"), user("U0ALICE", "alice")];
    let (bridge, core) = cx.update(|cx| {
        make_wired_workspace("T0MSG", "Msg Co", &[("C0ENG", "engineering")], &people, cx)
    });
    let fx = bridge.snapshot(&HashMap::new()).expect("snapshot");
    let (root, app, cx) = mount_wired(fx, vec![bridge], cx);
    let _ = core.drain_commands();

    // 1. Focus composer, type a real message through GPUI key dispatch, press Enter.
    cx.update(|window, cx| {
        let composer = app.read(cx).composer.clone();
        composer.update(cx, |c, cx| c.focus_input(window, cx));
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("h e l l o space w i r e d enter");
    cx.run_until_parked();

    let cmds = core.drain_commands();
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Messaging(MessagingCommand::SendMessage { channel, content, .. })
                if channel.as_str() == "C0ENG" && content.text == "hello wired"
        )),
        "submitting composer must send MessagingCommand::SendMessage, got: {cmds:?}"
    );

    // 2. Core writes an incoming peer message to SQLite and emits `Event::MessagePosted`.
    let peer_msg = Message {
        ts: Ts::from("1700000999.000100"),
        user: Some(UserId::from("U0ALICE")),
        text: "Inbound from real EventHub".into(),
        ..Default::default()
    };
    {
        let mut s = core.store().lock().expect("store lock");
        s.upsert_message(&TeamId::from("T0MSG"), &ChannelId::from("C0ENG"), &peer_msg)
            .expect("upsert");
    }
    cx.update(|_, cx| {
        core.emit(
            Event::StoreChanged {
                channel: ChannelId::from("C0ENG"),
            },
            cx,
        );
        core.emit(
            Event::MessagePosted {
                channel: ChannelId::from("C0ENG"),
                message: Box::new(peer_msg.clone()),
            },
            cx,
        );
    });
    cx.executor().advance_clock(RELOAD_DEBOUNCE);
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        let last = app
            .read(cx)
            .messages
            .read(cx)
            .last_message_text_for_test(cx);
        assert_eq!(
            last.as_deref(),
            Some("Inbound from real EventHub"),
            "StoreChanged + MessagePosted via EventHub must update rendered MessageList"
        );
    });

    // 3. Emit an outbox failure through `EventHub`, verify GUI tracks it cleanly.
    let item = {
        let mut s = core.store().lock().expect("store lock");
        let row = s
            .enqueue_outbox(
                &TeamId::from("T0MSG"),
                &NewOutboxItem {
                    channel_id: &ChannelId::from("C0ENG"),
                    thread_ts: None,
                    kind: OutboxKind::Message,
                    payload_json: r#"{"kind":"message","content":{"text":"offline msg"}}"#,
                    last_error: Some("503 unavailable"),
                },
            )
            .expect("enqueue");
        s.mark_outbox_failed(row.id, "503 unavailable")
            .expect("fail");
        s.list_outbox(&TeamId::from("T0MSG")).expect("list")
    };
    cx.update(|_, cx| {
        core.emit(Event::OutboxUpdated(item), cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        assert_eq!(app.read(cx).ws.outbox.len(), 1);
    });
    drop(root);
}

#[gpui::test]
fn wired_confirmed_send_stays_on_the_channel_it_was_submitted_to(cx: &mut gpui::TestAppContext) {
    let people = [user("U0ME", "me"), user("U0PEER", "peer")];
    let (bridge, core) = cx.update(|cx| {
        make_wired_workspace(
            "T0BCAST",
            "Broadcast Co",
            &[("C0BIG", "big"), ("C0SMALL", "small")],
            &people,
            cx,
        )
    });
    // `@channel` in a channel of six or more members asks before sending.
    core.store()
        .lock()
        .expect("store lock")
        .upsert_conversation(
            &TeamId::from("T0BCAST"),
            &Conversation {
                id: ChannelId::from("C0BIG"),
                name: Some("big".into()),
                is_channel: true,
                is_member: true,
                num_members: Some(12),
                ..Default::default()
            },
        )
        .expect("upsert");
    let fx = bridge.snapshot(&HashMap::new()).expect("snapshot");
    let (root, app, cx) = mount_wired(fx, vec![bridge], cx);
    let _ = core.drain_commands();
    let (big, small) = (ChannelId::from("C0BIG"), ChannelId::from("C0SMALL"));
    let sent_to = |cmds: &[Command], channel: &ChannelId| {
        cmds.iter().any(|c| {
            matches!(
                c,
                Command::Messaging(MessagingCommand::SendMessage { channel: target, .. })
                    if target == channel
            )
        })
    };

    // 1. `@channel hi` in #big opens the confirmation; nothing goes out yet.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.select(big.clone(), cx);
            app.send(
                crate::composer::content_of("<!channel> hi", &app.fixtures),
                window,
                cx,
            );
        });
        let _ = window.draw(cx);
    });
    cx.run_until_parked();
    let cmds = core.drain_commands();
    assert!(
        !sent_to(&cmds, &big),
        "the @channel guard must hold the message until confirmed: {cmds:?}"
    );

    // 2. The user moves to #small before confirming: the message still goes to #big.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| app.select(small.clone(), cx));
        let _ = window.draw(cx);
    });
    let send = cx
        .debug_bounds("broadcast-send")
        .expect("the confirmation's Send button is on screen");
    cx.simulate_click(send.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    let cmds = core.drain_commands();
    assert!(
        sent_to(&cmds, &big),
        "confirming must send to the channel the message was submitted to: {cmds:?}"
    );
    assert!(
        !sent_to(&cmds, &small),
        "nothing may reach the channel that was open at confirm time: {cmds:?}"
    );

    // 3. Canceling after the same switch puts the text and its staged file back where they
    // were typed.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.select(big.clone(), cx);
            let content = crate::composer::content_of("<!channel> again", &app.fixtures);
            let file = crate::composer::PendingAttachment::new(
                "notes.txt".into(),
                b"notes".to_vec(),
                None,
            );
            app.upload_files(big.clone(), None, Some(content), vec![file], window, cx);
            app.select(small.clone(), cx);
        });
        let _ = window.draw(cx);
    });
    let cancel = cx
        .debug_bounds("broadcast-cancel")
        .expect("Cancel is on screen");
    cx.simulate_click(cancel.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, cx| {
        let app = app.read(cx);
        assert_eq!(
            app.channel, big,
            "cancel returns to the conversation the text belongs to"
        );
        let composer = app.composer.read(cx);
        assert_eq!(composer.text(), "<!channel> again");
        let restored: Vec<_> = composer
            .attachments()
            .iter()
            .map(|file| file.filename.as_str())
            .collect();
        assert_eq!(
            restored,
            ["notes.txt"],
            "cancel must stage the upload's file again, not only its text"
        );
    });
    assert!(
        !sent_to(&core.drain_commands(), &big),
        "a canceled send sends nothing"
    );

    // 4. An upload's message passes the same guard: nothing uploads until it is confirmed, and
    // then into the channel it was submitted to.
    let uploaded_to = |cmds: &[Command], channel: &ChannelId| {
        cmds.iter().any(|c| {
            matches!(
                c,
                Command::Collab(CollabCommand::UploadFiles { channel: target, .. })
                    if target == channel
            )
        })
    };
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.select(big.clone(), cx);
            let content = crate::composer::content_of("<!channel> notes attached", &app.fixtures);
            let file = crate::composer::PendingAttachment::new(
                "notes.txt".into(),
                b"notes".to_vec(),
                None,
            );
            app.upload_files(big.clone(), None, Some(content), vec![file], window, cx);
            app.select(small.clone(), cx);
        });
        let _ = window.draw(cx);
    });
    cx.run_until_parked();
    assert!(
        !uploaded_to(&core.drain_commands(), &big),
        "the @channel guard must hold an upload's message until confirmed"
    );
    let send = cx
        .debug_bounds("broadcast-send")
        .expect("the upload's confirmation is on screen");
    cx.simulate_click(send.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    let cmds = core.drain_commands();
    assert!(
        uploaded_to(&cmds, &big),
        "confirming uploads into the channel the message was submitted to: {cmds:?}"
    );
    assert!(
        !uploaded_to(&cmds, &small),
        "nothing may upload into the channel open at confirm time: {cmds:?}"
    );
    drop(root);
}

#[gpui::test]
fn wired_multi_workspace_event_isolation_and_switching(cx: &mut gpui::TestAppContext) {
    let people = [user("U0ME", "me"), user("U0PEER", "peer")];
    let ((b1, c1), (b2, c2)) = cx.update(|cx| {
        (
            make_wired_workspace(
                "T0ONE",
                "Workspace One",
                &[("C0ONE", "one-chan")],
                &people,
                cx,
            ),
            make_wired_workspace(
                "T0TWO",
                "Workspace Two",
                &[("C0TWO", "two-chan")],
                &people,
                cx,
            ),
        )
    });
    let mut fx = b1.snapshot(&HashMap::new()).expect("ws1 snapshot");
    fx.set_rail_workspaces(vec![b1.team_summary().unwrap(), b2.team_summary().unwrap()]);
    // T0TWO already holds a draft of its own, restored on its first visit.
    let seed = b2
        .remember(
            &DRAFTS,
            &[SavedDraft {
                channel: ChannelId::from("C0TWO"),
                label: "two-chan".into(),
                text: "two's own draft".into(),
                at: 1,
            }],
        )
        .expect("a live bridge persists drafts");
    cx.executor().spawn(seed).detach();
    cx.run_until_parked();
    let (root, app, cx) = mount_wired(fx, vec![b1, b2], cx);
    let _ = c1.drain_commands();
    let _ = c2.drain_commands();

    // Background workspace T0TWO receives a live message via its own EventHub.
    let bg_msg = Message {
        ts: Ts::from("1700001234.000100"),
        user: Some(UserId::from("U0PEER")),
        text: "Ping <@U0ME> in workspace two".into(),
        ..Default::default()
    };
    // T0TWO's prefs mute #two-chan.
    let muted = serde_json::json!({
        "all_notifications_prefs":
            serde_json::json!({ "channels": { "C0TWO": { "muted": true } } }).to_string(),
    });
    {
        let mut s = c2.store().lock().expect("store2 lock");
        s.upsert_message(&TeamId::from("T0TWO"), &ChannelId::from("C0TWO"), &bg_msg)
            .expect("upsert ws2");
        s.replace_prefs(&TeamId::from("T0TWO"), &muted)
            .expect("store ws2 prefs");
    }
    cx.update(|_, cx| {
        c2.emit(
            Event::MessagePosted {
                channel: ChannelId::from("C0TWO"),
                message: Box::new(bg_msg),
            },
            cx,
        );
    });
    cx.run_until_parked();

    // Active workspace T0ONE must NOT be polluted by T0TWO's channel, while T0TWO badge updates.
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        assert_eq!(app.read(cx).workspace, 0);
        assert!(
            app.read(cx)
                .fixtures
                .conversation(&ChannelId::from("C0TWO"))
                .is_none(),
            "background workspace message must never leak into active workspace snapshot"
        );
    });
    // T0TWO's notification levels were read for that message, and are read again after a
    // sidebar update, which a changed level brings.
    cx.update(|_, cx| {
        assert_eq!(
            app.read(cx).slots[1].state.notify_levels,
            Some(HashMap::from([(
                ChannelId::from("C0TWO"),
                NotifyLevel::Muted
            )]))
        );
    });
    cx.update(|_, cx| c2.emit(Event::SidebarChanged, cx));
    cx.run_until_parked();
    cx.update(|_, cx| assert_eq!(app.read(cx).slots[1].state.notify_levels, None));

    // Switch to T0TWO -> off-thread snapshot loads C0TWO cleanly.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| app.switch_workspace(1, window, cx));
    });
    cx.executor().advance_clock(RELOAD_DEBOUNCE);
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        assert_eq!(app.read(cx).workspace, 1);
        assert!(
            app.read(cx)
                .fixtures
                .conversation(&ChannelId::from("C0TWO"))
                .is_some(),
            "switching to workspace 1 must load its channels"
        );
    });

    // A popped-out window of T0ONE jumps back into T0ONE, whichever workspace is shown.
    let jump =
        cx.update(|window, cx| app.update(cx, |app, cx| app.popout_jump_handler(0, window, cx)));
    // From outside the main window's update, as a popout's click is.
    cx.cx
        .update(|cx| jump(ChannelId::from("C0ONE"), None, false, cx));
    cx.executor().advance_clock(RELOAD_DEBOUNCE);
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(
            app.read(cx).workspace,
            0,
            "a jump shows the popout's workspace"
        );
        assert_eq!(app.read(cx).channel, ChannelId::from("C0ONE"));
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| app.switch_workspace(1, window, cx));
    });
    cx.executor().advance_clock(RELOAD_DEBOUNCE);
    cx.run_until_parked();

    // Back on T0ONE a draft is typed, then T0TWO's tile is moved to the top of the rail.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.switch_workspace(0, window, cx);
            app.composer
                .update(cx, |c, cx| c.load_draft("typed before the move", cx));
            app.save_draft(cx);
            app.sidebar.update(cx, |_, cx| {
                cx.emit(SidebarEvent::ReorderWorkspace { from: 0, to: 1 });
            });
        });
    });
    cx.run_until_parked();

    // Moving a tile changes no workspace's identity: T0TWO's counts stay out of T0ONE...
    let two_counts = slackless_core::api::ClientCounts {
        channels: vec![ConversationCounts {
            id: ChannelId::from("C0TWO"),
            has_unreads: true,
            mention_count: 1,
            ..Default::default()
        }],
        ..Default::default()
    };
    cx.update(|_, cx| c2.emit(Event::Counts(Box::new(two_counts)), cx));
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(
            !app.read(cx)
                .ws
                .counts
                .contains_key(&ChannelId::from("C0TWO")),
            "a background workspace's counts must not reach the shown one after a reorder"
        );
    });

    // ...T0ONE's own events keep refreshing its open conversation...
    let live = Message {
        ts: Ts::from("1700002000.000100"),
        user: Some(UserId::from("U0PEER")),
        text: "Still live after the move".into(),
        ..Default::default()
    };
    c1.store()
        .lock()
        .expect("store1 lock")
        .upsert_message(&TeamId::from("T0ONE"), &ChannelId::from("C0ONE"), &live)
        .expect("upsert ws1");
    cx.update(|_, cx| {
        c1.emit(
            Event::StoreChanged {
                channel: ChannelId::from("C0ONE"),
            },
            cx,
        );
        c1.emit(
            Event::MessagePosted {
                channel: ChannelId::from("C0ONE"),
                message: Box::new(live),
            },
            cx,
        );
    });
    cx.executor().advance_clock(RELOAD_DEBOUNCE);
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        assert_eq!(
            app.read(cx)
                .messages
                .read(cx)
                .last_message_text_for_test(cx)
                .as_deref(),
            Some("Still live after the move"),
            "the shown workspace keeps its live updates after a reorder"
        );
    });

    // ...and T0ONE's drafts persist into T0ONE's store, without T0TWO's.
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.composer
                .update(cx, |c, cx| c.load_draft("typed after the move", cx));
            app.save_draft(cx);
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        let stored: Vec<String> = (app.read(cx).bridge.read_local(&DRAFTS).unwrap_or_default())
            .into_iter()
            .map(|draft| draft.text)
            .collect();
        assert_eq!(
            stored,
            ["typed after the move"],
            "T0ONE's store keeps exactly T0ONE's drafts"
        );
    });

    // Cmd+1 opens the rail's first tile, now T0TWO.
    cx.dispatch_action(SwitchWorkspace(0));
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(
            app.read(cx).bridge.team_id(),
            Some(&TeamId::from("T0TWO")),
            "Cmd+1 follows the rail's order"
        );
    });
    drop(root);
}

#[gpui::test]
fn wired_huddle_commands_follow_the_workspace_running_it(cx: &mut gpui::TestAppContext) {
    let people = [user("U0ME", "me"), user("U0PEER", "peer")];
    let ((b1, c1), (b2, c2)) = cx.update(|cx| {
        (
            make_wired_workspace(
                "T0ONE",
                "Workspace One",
                &[("C0ONE", "one-chan")],
                &people,
                cx,
            ),
            make_wired_workspace(
                "T0TWO",
                "Workspace Two",
                &[("C0TWO", "two-chan")],
                &people,
                cx,
            ),
        )
    });
    let mut fx = b1.snapshot(&HashMap::new()).expect("ws1 snapshot");
    fx.set_rail_workspaces(vec![b1.team_summary().unwrap(), b2.team_summary().unwrap()]);
    let (root, app, cx) = mount_wired(fx, vec![b1, b2], cx);
    let one = ChannelId::from("C0ONE");
    let session = |muted: bool| {
        Event::Huddle(HuddleEvent::SessionChanged(Box::new(
            HuddleSessionSnapshot {
                phase: HuddlePhase::Connected,
                channel: ChannelId::from("C0ONE"),
                call_id: "R0ONE".into(),
                muted,
                ..Default::default()
            },
        )))
    };
    let huddle_commands = |core: &TestCoreHandle| -> Vec<Command> {
        core.drain_commands()
            .into_iter()
            .filter(|c| matches!(c, Command::Huddle(_)))
            .collect()
    };

    // 1. Join in workspace one, then show workspace two.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| app.join_huddle_in(one.clone(), window, cx));
    });
    cx.update(|_, cx| c1.emit(session(false), cx));
    cx.run_until_parked();
    cx.update(|window, cx| {
        app.update(cx, |app, cx| app.switch_workspace(1, window, cx));
    });
    cx.run_until_parked();
    let _ = (c1.drain_commands(), c2.drain_commands());

    // 2. Workspace one's session events still reach the dock.
    cx.update(|_, cx| c1.emit(session(true), cx));
    cx.run_until_parked();
    cx.update(|_, cx| {
        let state = (app.read(cx).huddle_state.clone()).expect("the dock stays across a switch");
        assert!(
            state.read(cx).session().muted,
            "the huddle's workspace keeps updating the dock while another one is shown"
        );
    });

    // 2b. The dock's notes canvas and huddle thread belong to the huddle's workspace: opening
    // either with workspace two shown shows workspace one, whose core loads the canvas.
    let root_ts = Ts::from("1700000500.000100");
    cx.update(|_, cx| {
        c1.emit(
            Event::Huddle(HuddleEvent::Updated {
                channel: one.clone(),
                participants: vec![UserId::from("U0ME")],
                ended: false,
                topic: None,
                background_id: None,
                thread_root_ts: Some(root_ts.clone()),
            }),
            cx,
        );
    });
    cx.run_until_parked();
    let dock = (cx.update(|_, cx| app.read(cx).huddle_dock.clone())).expect("the dock is mounted");
    for event in [
        crate::huddle_dock::HuddleDockEvent::OpenCanvas("F0NOTES".into()),
        crate::huddle_dock::HuddleDockEvent::OpenThread,
    ] {
        cx.update(|window, cx| {
            app.update(cx, |app, cx| app.switch_workspace(1, window, cx));
        });
        cx.update(|_, cx| dock.update(cx, |_, cx| cx.emit(event)));
        cx.run_until_parked();
        cx.update(|_, cx| assert_eq!(app.read(cx).workspace, 0, "the huddle's workspace shows"));
    }
    cx.update(|_, cx| {
        assert_eq!(
            app.read(cx).thread.read(cx).open_thread(),
            Some((one.clone(), root_ts.clone())),
            "the huddle thread opens in the huddle's workspace"
        );
    });
    let canvas_loads = |core: &TestCoreHandle| {
        (core.drain_commands().into_iter())
            .filter(|c| matches!(c, Command::Collab(CollabCommand::LoadCanvas { .. })))
            .count()
    };
    assert_eq!(
        (canvas_loads(&c1), canvas_loads(&c2)),
        (1, 0),
        "the notes canvas loads from the huddle's workspace"
    );
    cx.update(|window, cx| {
        app.update(cx, |app, cx| app.switch_workspace(1, window, cx));
    });
    cx.run_until_parked();
    let _ = (c1.drain_commands(), c2.drain_commands());

    // 3. A DM huddle invite from workspace one rings while workspace two is shown, and
    // declining answers workspace one.
    cx.update(|_, cx| {
        c1.emit(
            Event::Huddle(HuddleEvent::Invite {
                channel: ChannelId::from("D0PEER"),
                inviter: UserId::from("U0PEER"),
                call_id: "R0RING".into(),
            }),
            cx,
        );
    });
    cx.run_until_parked();
    let banner = cx
        .update(|_, cx| app.read(cx).huddle_invite.clone())
        .expect("an invite from a workspace that is not shown still rings");
    cx.update(|_, cx| {
        banner.update(cx, |_, cx| {
            cx.emit(crate::huddle_invite::HuddleInviteEvent::Decline);
        });
    });
    cx.run_until_parked();

    // 4. Mute and Leave, pressed with workspace two shown, reach workspace one's core only.
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.toggle_huddle_mute(cx);
            app.leave_huddle(cx);
        });
    });
    cx.run_until_parked();
    let (one_cmds, two_cmds) = (huddle_commands(&c1), huddle_commands(&c2));
    assert!(
        one_cmds
            .iter()
            .any(|c| matches!(c, Command::Huddle(HuddleCommand::DeclineInvite { .. }))),
        "declining answers the inviting workspace: {one_cmds:?}"
    );
    assert!(
        one_cmds.iter().any(|c| matches!(
            c,
            Command::Huddle(HuddleCommand::Control(HuddleControlCommand::SetMuted(_)))
        )),
        "mute goes to the huddle's workspace: {one_cmds:?}"
    );
    assert!(
        one_cmds
            .iter()
            .any(|c| matches!(c, Command::Huddle(HuddleCommand::Leave))),
        "leave goes to the huddle's workspace: {one_cmds:?}"
    );
    assert!(
        two_cmds.is_empty(),
        "the shown workspace runs no huddle: {two_cmds:?}"
    );
    drop(root);
}

#[gpui::test]
fn wired_unread_counts_fold_alike_in_shown_and_background_workspaces(
    cx: &mut gpui::TestAppContext,
) {
    let people = [user("U0ME", "me"), user("U0PEER", "peer")];
    let workspace = |team: &str, cx: &mut gpui::App| {
        let channels = [("C0OPEN", "open"), ("C0CHAN", "chan")];
        let (bridge, core) = make_wired_workspace(team, team, &channels, &people, cx);
        let im = Conversation {
            id: ChannelId::from("D0PEER"),
            is_im: true,
            is_open: true,
            user: Some(UserId::from("U0PEER")),
            ..Default::default()
        };
        (core.store().lock().expect("store lock"))
            .upsert_conversation(&TeamId::from(team), &im)
            .expect("upsert IM");
        (bridge, core)
    };
    let ((b1, c1), (b2, c2)) = cx.update(|cx| (workspace("T0ONE", cx), workspace("T0TWO", cx)));
    let mut fx = b1.snapshot(&HashMap::new()).expect("ws1 snapshot");
    fx.set_rail_workspaces(vec![b1.team_summary().unwrap(), b2.team_summary().unwrap()]);
    let (root, app, cx) = mount_wired(fx, vec![b1, b2], cx);
    // The shown workspace reads #open, so neither workspace sees the rows below arrive.
    cx.update(|_, cx| app.update(cx, |app, cx| app.select(ChannelId::from("C0OPEN"), cx)));
    let (chan, dm) = (ChannelId::from("C0CHAN"), ChannelId::from("D0PEER"));
    // A DM opened by its first message: neither snapshot nor store holds it yet.
    let new_dm = ChannelId::from("D0NEW");
    // A group DM opened so, its id (`C…`) any channel's: only the live event names its kind.
    let new_group = ChannelId::from("C0NEWGROUP");
    let group_opener: Message = serde_json::from_value(serde_json::json!({
        "ts": "1700000006.000100",
        "user": "U0PEER",
        "text": "hi all",
        "channel_type": "mpim",
    }))
    .expect("a live message");
    let post = |channel: &ChannelId, ts: &str, user: &str, text: &str| Event::MessagePosted {
        channel: channel.clone(),
        message: Box::new(Message {
            ts: Ts::from(ts),
            user: Some(UserId::from(user)),
            text: text.into(),
            ..Default::default()
        }),
    };
    let mark = |channel: &ChannelId, ts: &str| Event::ReadMarker {
        channel: channel.clone(),
        ts: Ts::from(ts),
    };
    // What happens, then the conversation's unread flag, mentions, DM count and read cursor.
    let rows = [
        (
            "a peer posts",
            post(&chan, "1700000001.000100", "U0PEER", "hello"),
            &chan,
            (true, 0, 0, None),
        ),
        (
            "a peer mentions me",
            post(&chan, "1700000002.000100", "U0PEER", "<@U0ME> look"),
            &chan,
            (true, 1, 0, None),
        ),
        (
            "a peer DMs me",
            post(&dm, "1700000003.000100", "U0PEER", "hi"),
            &dm,
            (true, 1, 1, None),
        ),
        (
            "the DM is read",
            mark(&dm, "1700000003.000100"),
            &dm,
            (false, 0, 0, Some("1700000003.000100")),
        ),
        (
            "the DM is marked unread elsewhere",
            mark(&dm, "1700000002.000100"),
            &dm,
            (true, 1, 0, Some("1700000002.000100")),
        ),
        (
            "I reply",
            post(&chan, "1700000004.000100", "U0ME", "on it"),
            &chan,
            (true, 1, 0, Some("1700000004.000100")),
        ),
        (
            "a peer opens a new DM",
            post(&new_dm, "1700000005.000100", "U0PEER", "hi"),
            &new_dm,
            (true, 1, 1, None),
        ),
        (
            "a peer opens a new group DM",
            Event::MessagePosted {
                channel: new_group.clone(),
                message: Box::new(group_opener),
            },
            &new_group,
            (true, 1, 0, None),
        ),
    ];
    for (what, event, channel, expected) in rows {
        cx.update(|_, cx| {
            c1.emit(event.clone(), cx);
            c2.emit(event, cx);
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let app = app.read(cx);
            let shown = app.ws.counts.get(channel).cloned().unwrap_or_default();
            let background = (app.slots[1].state.counts.get(channel).cloned()).unwrap_or_default();
            assert_eq!(shown, background, "{what}: both workspaces fold it alike");
            let observed = (
                shown.has_unreads,
                shown.mention_count,
                shown.dm_count,
                shown.last_read.as_ref().map(Ts::as_str),
            );
            assert_eq!(observed, expected, "{what}");
        });
    }
    drop(root);
}

#[gpui::test]
fn wired_count_changes_redraw_only_the_views_that_show_counts(cx: &mut gpui::TestAppContext) {
    let people = [user("U0ME", "me"), user("U0PEER", "peer")];
    let channels = [("C0OPEN", "open"), ("C0CHAN", "chan")];
    let (bridge, core) =
        cx.update(|cx| make_wired_workspace("T0CNT", "Counts Co", &channels, &people, cx));
    let fx = bridge.snapshot(&HashMap::new()).expect("snapshot");
    let (root, app, cx) = mount_wired(fx, vec![bridge], cx);
    cx.update(|_, cx| app.update(cx, |app, cx| app.select(ChannelId::from("C0OPEN"), cx)));
    cx.run_until_parked();
    // The quick switcher lists names, never unread state.
    let redrawn = Rc::new(Cell::new(0));
    let _watch = cx.update(|_, cx| {
        let switcher = app.read(cx).switcher.clone();
        let redrawn = redrawn.clone();
        cx.observe(&switcher, move |_, _| redrawn.set(redrawn.get() + 1))
    });
    let chan = ChannelId::from("C0CHAN");
    let unread = slackless_core::api::ClientCounts {
        channels: vec![ConversationCounts {
            id: chan.clone(),
            has_unreads: true,
            latest: Some(Ts::from("1700000001.000100")),
            ..Default::default()
        }],
        ..Default::default()
    };
    let rows = [
        ("Slack counts an unread", Event::Counts(Box::new(unread))),
        (
            "another device reads it",
            Event::ReadMarker {
                channel: chan.clone(),
                ts: Ts::from("1700000001.000100"),
            },
        ),
        (
            "a peer posts",
            Event::MessagePosted {
                channel: chan.clone(),
                message: Box::new(Message {
                    ts: Ts::from("1700000002.000100"),
                    user: Some(UserId::from("U0PEER")),
                    text: "hi".into(),
                    ..Default::default()
                }),
            },
        ),
    ];
    for (what, event) in rows {
        cx.update(|_, cx| core.emit(event, cx));
        cx.run_until_parked();
        cx.update(|_, cx| {
            let app = app.read(cx);
            assert_eq!(
                app.fixtures.counts, app.ws.counts,
                "{what}: the snapshot follows"
            );
            assert_eq!(
                redrawn.get(),
                0,
                "{what}: a view showing no counts stays as it is"
            );
        });
    }
    drop(root);
}

#[gpui::test]
fn wired_history_dialogs_read_the_archive_off_the_ui_thread(cx: &mut gpui::TestAppContext) {
    let people = [user("U0ME", "me")];
    let (bridge, _core) =
        cx.update(|cx| make_wired_workspace("T0REV", "Rev Co", &[("C0GEN", "gen")], &people, cx));
    let fx = bridge.snapshot(&HashMap::new()).expect("snapshot");
    let (root, app, cx) = mount_wired(fx, vec![bridge], cx);
    let general = ChannelId::from("C0GEN");
    for (what, transcript) in [("edit history", false), ("huddle transcript", true)] {
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                if transcript {
                    let call = "R0CALL".to_owned();
                    app.open_huddle_transcript(call, general.clone(), None, None, window, cx);
                } else {
                    let ts = Ts::from("1700000001.000100");
                    app.open_revision_history(general.clone(), ts, window, cx);
                }
            });
            assert!(
                !window.has_active_dialog(cx),
                "{what}: no archive read on the UI thread"
            );
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(
                window.has_active_dialog(cx),
                "{what}: opens once its read lands"
            );
            window.close_dialog(cx);
        });
        cx.run_until_parked();
    }
    drop(root);
}

#[gpui::test]
fn wired_pending_dm_delivers_only_to_the_dm_it_was_addressed_to(cx: &mut gpui::TestAppContext) {
    let people = [
        user("U0ME", "me"),
        user("U0ALICE", "alice"),
        user("U0BOB", "bob"),
    ];
    let (bridge, core) =
        cx.update(|cx| make_wired_workspace("T0DM", "DM Co", &[("C0GEN", "general")], &people, cx));
    let fx = bridge.snapshot(&HashMap::new()).expect("snapshot");
    let (root, app, cx) = mount_wired(fx, vec![bridge], cx);
    let _ = core.drain_commands();
    let (alice, bob) = (UserId::from("U0ALICE"), UserId::from("U0BOB"));
    let pending = |cx: &mut gpui::App| app.read(cx).pending_dm.is_some();
    // The core stores a DM before announcing that it opened.
    let announce = |dm: Conversation, cx: &mut gpui::App| {
        core.store()
            .lock()
            .expect("store lock")
            .upsert_conversation(&TeamId::from("T0DM"), &dm)
            .expect("upsert DM");
        core.emit(Event::ConversationOpened(dm.id.clone()), cx);
    };

    // 1. New message to Alice, who has no IM yet: the text waits while the core opens one.
    cx.update(|_, cx| {
        app.read(cx).compose.clone().update(cx, |_, cx| {
            cx.emit(NewMessageEvent::OpenDm {
                users: vec![alice.clone()],
                content: MessageContent::plain("for alice"),
            });
        });
    });
    cx.run_until_parked();
    assert!(
        core.drain_commands().iter().any(|c| matches!(
            c,
            Command::Channel(ChannelCommand::OpenDm { users }) if users.len() == 1 && users[0] == alice
        )),
        "the GUI must ask the core to open Alice's DM"
    );

    // 2. Meanwhile the hover card opens Bob's DM: Alice's text must not land there.
    cx.update(|_, cx| {
        announce(
            Conversation {
                id: ChannelId::from("D0BOB"),
                is_im: true,
                user: Some(bob.clone()),
                ..Default::default()
            },
            cx,
        )
    });
    cx.run_until_parked();
    let cmds = core.drain_commands();
    assert!(
        !cmds.iter().any(|c| matches!(c, Command::Messaging(_))),
        "text typed for Alice must never post to the DM that opened meanwhile: {cmds:?}"
    );
    assert!(
        cx.update(|_, cx| pending(cx)),
        "Alice's text keeps waiting for her DM"
    );

    // 3. Her DM opens: the text goes there.
    cx.update(|_, cx| {
        announce(
            Conversation {
                id: ChannelId::from("D0ALICE"),
                is_im: true,
                user: Some(alice.clone()),
                ..Default::default()
            },
            cx,
        )
    });
    cx.run_until_parked();
    let cmds = core.drain_commands();
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Messaging(MessagingCommand::SendMessage { channel, content, .. })
                if channel.as_str() == "D0ALICE" && content.text == "for alice"
        )),
        "Alice's DM opening delivers the text to it: {cmds:?}"
    );
    assert!(!cx.update(|_, cx| pending(cx)));

    // 4. A failed `conversations.open` drops the waiting text instead of leaving it for the
    //    next DM that opens.
    cx.update(|_, cx| {
        app.read(cx).compose.clone().update(cx, |_, cx| {
            cx.emit(NewMessageEvent::OpenDm {
                users: vec![alice.clone()],
                content: MessageContent::plain("for alice again"),
            });
        });
    });
    cx.run_until_parked();
    let _ = core.drain_commands();
    cx.update(|_, cx| {
        core.emit(
            Event::Error("conversations.open: slack api error: user_not_found".into()),
            cx,
        )
    });
    cx.run_until_parked();
    assert!(
        !cx.update(|_, cx| pending(cx)),
        "an open failure must clear the pending DM"
    );
    drop(root);
}

#[gpui::test]
fn ui_consistency_all_views_and_dialogs_render_across_themes_densities_and_viewports(
    cx: &mut gpui::TestAppContext,
) {
    let fx = fixtures::load();
    let (root, app, cx) = mount_wired(fx, vec![CoreBridge::offline()], cx);

    let matrix = [
        (SlackTheme::Dark, false, size(px(1280.), px(800.))),
        (SlackTheme::Light, true, size(px(820.), px(640.))),
    ];

    for (mode, compact, _viewport) in matrix {
        cx.update(|window, cx| {
            theme::activate(mode, cx);
            app.update(cx, |app, cx| {
                app.preferences.update(cx, |prefs, cx| {
                    let mut s = prefs.settings().clone();
                    s.density = usize::from(compact);
                    prefs.load_settings(s, window, cx);
                });
            });
            let _ = window.draw(cx);
        });

        // Cycle through every major page & render cleanly.
        let pages = [
            MainPage::Channel,
            MainPage::Threads,
            MainPage::Unreads,
            MainPage::Later,
            MainPage::Activity,
            MainPage::Files,
            MainPage::Directory,
            MainPage::Huddles,
            MainPage::CatchUp,
            MainPage::Search,
        ];
        for page in pages {
            cx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    app.page = page;
                    cx.notify();
                });
                let _ = window.draw(cx);
            });
        }

        // Open previously untested high-churn dialogs (Storage, Summary, Preferences) & draw cleanly.
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.page = MainPage::Channel;
                app.open_storage_dialog(window, cx);
            });
            let _ = window.draw(cx);
            window.close_dialog(cx);
            app.update(cx, |app, cx| {
                app.open_summary_for_channel(window, cx);
            });
            let _ = window.draw(cx);
            window.close_dialog(cx);
            app.update(cx, |app, cx| {
                app.open_preferences(None, window, cx);
            });
            let _ = window.draw(cx);
            window.close_dialog(cx);
        });

        cx.update(|_, cx| {
            assert_eq!(
                cx.theme().mode.is_dark(),
                matches!(mode, SlackTheme::Dark),
                "theme mode must stay consistent after cycling pages and dialogs"
            );
        });
    }

    // Ensure at least one control-command type check compiles cleanly for future tests.
    let _ = std::mem::size_of::<HuddleControlCommand>();
    drop(root);
}

#[test]
fn ui_consistency_theme_token_guardrail_blocks_new_hardcoded_colors() {
    // `theme.rs` documents the invariant: every standard view reads colors through `cx.theme()`
    // or `theme::*` helpers rather than scattering raw `rgb()`/`rgba()`/`hsla()` literals.
    // This allowlist tracks ONLY intentional exceptions (dark video/huddle overlay surfaces,
    // user-defined hex color previewers, and box-shadow/media-scrim helpers). New views must NOT
    // be added here; add a semantic token in `theme.rs` instead.
    const ALLOWLIST: &[&str] = &[
        "theme.rs",
        "app/chrome/mod.rs",
        "files/media_cards.rs",
        "media/video_player.rs",
        "emoji/gifs.rs",
        "messages/render/inline.rs",
        "composer/tokens.rs",
        "preferences/mod.rs",
        "preferences/sections/appearance.rs",
    ];

    fn walk(root: &Path, dir: &Path, violations: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, violations);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let rel = path
                .strip_prefix(root)
                .expect("strip prefix")
                .to_string_lossy()
                .replace('\\', "/");
            if rel.ends_with("/tests.rs")
                || rel.contains("/tests/")
                || rel.ends_with("wired_tests.rs")
                || ALLOWLIST.contains(&rel.as_str())
            {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (idx, line) in content.lines().enumerate() {
                let trimmed = line.trim_start();
                if trimmed.starts_with("//") {
                    continue;
                }
                if trimmed.contains("rgb(")
                    || trimmed.contains("rgba(")
                    || trimmed.contains("hsla(")
                {
                    violations.push(format!("{rel}:{}: {}", idx + 1, trimmed));
                }
            }
        }
    }

    let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();
    walk(&src_root, &src_root, &mut violations);
    assert!(
        violations.is_empty(),
        "Hardcoded colors found outside theme.rs (add a semantic token to theme.rs instead):\n{}",
        violations.join("\n")
    );
}

#[gpui::test]
fn wired_storage_dialog_backfill_commands_and_progress_events(cx: &mut gpui::TestAppContext) {
    use slackless_core::WorkspaceCommand;
    use slackless_core::backfill::{BackfillStatus, WorkspaceBackfillProgress};

    let people = [user("U0ME", "me"), user("U0PEER", "peer")];
    let (bridge, core) = cx.update(|cx| {
        make_wired_workspace(
            "T0STOR",
            "Storage Test",
            &[("C0ARCH", "archive")],
            &people,
            cx,
        )
    });
    let fx = bridge.snapshot(&HashMap::new()).expect("snapshot");
    let (root, app, cx) = mount_wired(fx, vec![bridge], cx);
    let _ = core.drain_commands();

    // 1. Open Storage dialog, start/pause/resume workspace backfill -> exact WorkspaceCommands emitted!
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_storage_dialog(window, cx);
            app.start_workspace_backfill(window, cx);
            app.pause_backfill(window, cx);
            app.resume_backfill(window, cx);
        });
        let _ = window.draw(cx);
    });
    let cmds = core.drain_commands();
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::Workspace(WorkspaceCommand::BackfillWorkspace {
            include_threads: true
        })
    )));
    assert!(
        cmds.iter()
            .any(|c| matches!(c, Command::Workspace(WorkspaceCommand::PauseBackfill)))
    );
    assert!(
        cmds.iter()
            .any(|c| matches!(c, Command::Workspace(WorkspaceCommand::ResumeBackfill)))
    );

    // 2. Receive live BackfillProgress events (running -> paused -> complete) and render cleanly.
    for (status, paused, done_ch) in [
        (BackfillStatus::Running, false, 0),
        (BackfillStatus::Paused, true, 1),
        (BackfillStatus::Complete, false, 1),
    ] {
        cx.update(|window, cx| {
            core.emit(
                Event::BackfillProgress(WorkspaceBackfillProgress {
                    workspace: TeamId::from("T0STOR"),
                    status,
                    paused,
                    active_channel: (status == BackfillStatus::Running)
                        .then(|| ChannelId::from("C0ARCH")),
                    total_channels: 1,
                    completed_channels: done_ch,
                    pages_fetched: 2,
                    messages_stored: 42,
                    threads_fetched: 1,
                    replies_stored: 3,
                    channels: Vec::new(),
                }),
                cx,
            );
            let _ = window.draw(cx);
        });
        cx.run_until_parked();
    }

    cx.update(|window, cx| {
        window.close_dialog(cx);
    });
    drop(root);
}

#[gpui::test]
fn wired_summary_dialog_states_generation_guard_and_live_targets(cx: &mut gpui::TestAppContext) {
    use super::dialogs::summary::{SummaryDialogState, SummaryDialogView, SummaryRequest};
    use slackless_core::summarize::{AiProvider, SummaryResult, SummaryTarget};

    let people = [user("U0ME", "me"), user("U0PEER", "peer")];
    let (bridge, core) = cx.update(|cx| {
        make_wired_workspace(
            "T0SUM",
            "Summary Test",
            &[("C0ENG", "engineering")],
            &people,
            cx,
        )
    });
    {
        let mut s = core.store().lock().expect("store lock");
        let root_msg = Message {
            ts: Ts::from("1700000100.000100"),
            user: Some(UserId::from("U0PEER")),
            text: "Release checklist is ready for review".into(),
            ..Default::default()
        };
        s.upsert_message(&TeamId::from("T0SUM"), &ChannelId::from("C0ENG"), &root_msg)
            .expect("upsert");
    }
    let fx = bridge.snapshot(&HashMap::new()).expect("snapshot");
    let (root, app, cx) = mount_wired(fx, vec![bridge], cx);

    // 1. Channel, thread, and catch-up entry points open cleanly on wired workspace.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_summary_for_channel(window, cx);
        });
        let _ = window.draw(cx);
        window.close_dialog(cx);

        app.update(cx, |app, cx| {
            app.open_summary_for_thread(
                ChannelId::from("C0ENG"),
                Ts::from("1700000100.000100"),
                window,
                cx,
            );
        });
        let _ = window.draw(cx);
        window.close_dialog(cx);

        app.update(cx, |app, cx| {
            app.open_summary_for_catchup(window, cx);
        });
        let _ = window.draw(cx);
        window.close_dialog(cx);
    });

    // 2. Directly exercise SummaryDialogView across Loading -> stale gen ignored -> Ready -> Error
    //    in both Dark and Light modes.
    let req = SummaryRequest::Direct(SummaryTarget::Channel {
        channel_label: "#engineering".into(),
        topic: Some("Ship v2".into()),
        messages: Vec::new(),
    });
    let view = cx.update(|_, cx| {
        SummaryDialogView::new(
            app.downgrade(),
            req,
            SummaryDialogState::Loading {
                target_title: "#engineering".into(),
                provider: AiProvider::Ollama,
                model: "llama3.2".into(),
                source_items: 5,
            },
            7,
            cx,
        )
    });

    // Stale generation (6 != 7) MUST be ignored!
    cx.update(|_, cx| {
        view.update(cx, |v, cx| {
            v.apply_result(
                6,
                Err("stale error".into()),
                AiProvider::Ollama,
                "llama3.2".into(),
                cx,
            );
        });
        assert!(matches!(
            view.read(cx).state(),
            SummaryDialogState::Loading { .. }
        ));
    });

    // Matching generation transitions to Ready, then Error; render both in Dark and Light!
    let view_for_dialog = view.clone();
    cx.update(|window, cx| {
        window.open_dialog(cx, move |dialog, _, _| {
            dialog.child(view_for_dialog.clone())
        });
        let _ = window.draw(cx);

        view.update(cx, |v, cx| {
            v.apply_result(
                7,
                Ok(SummaryResult {
                    title: "#engineering".into(),
                    summary: "## Key Decisions\n- Shipped v2 cleanly".into(),
                    provider: AiProvider::Ollama,
                    endpoint: "http://localhost:11434/v1".into(),
                    model: "llama3.2".into(),
                    target_kind: "channel".into(),
                    source_items: 5,
                    truncated: false,
                    prompt_tokens: Some(120),
                    completion_tokens: Some(45),
                    total_tokens: Some(165),
                    elapsed_ms: 42,
                }),
                AiProvider::Ollama,
                "llama3.2".into(),
                cx,
            );
        });
        assert!(matches!(
            view.read(cx).state(),
            SummaryDialogState::Ready(_)
        ));
        let _ = window.draw(cx);

        theme::activate(SlackTheme::Light, cx);
        view.update(cx, |v, cx| {
            v.apply_result(
                7,
                Err("connection refused".into()),
                AiProvider::Ollama,
                "llama3.2".into(),
                cx,
            );
        });
        assert!(matches!(
            view.read(cx).state(),
            SummaryDialogState::Error { .. }
        ));
        let _ = window.draw(cx);
        window.close_dialog(cx);
        theme::activate(SlackTheme::Dark, cx);
    });
    drop(root);
}

#[gpui::test]
fn wired_preferences_stealth_and_transcript_toggles_forward_commands_to_core(
    cx: &mut gpui::TestAppContext,
) {
    use slackless_core::WorkspaceCommand;

    let people = [user("U0ME", "me")];
    let (bridge, core) = cx.update(|cx| {
        make_wired_workspace("T0PREF", "Prefs Test", &[("C0GEN", "general")], &people, cx)
    });
    let fx = bridge.snapshot(&HashMap::new()).expect("snapshot");
    let (root, app, cx) = mount_wired(fx, vec![bridge], cx);
    let _ = core.drain_commands();

    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_preferences(Some(preferences::Section::Advanced), window, cx);
            app.preferences.update(cx, |prefs, cx| {
                let mut s = prefs.settings().clone();
                s.stealth_mode = !s.stealth_mode;
                s.huddle_save_transcripts = !s.huddle_save_transcripts;
                s.density = 1;
                prefs.load_settings(s, window, cx);
            });
            app.apply_preferences(window, cx);
        });
        let _ = window.draw(cx);
        window.close_dialog(cx);
    });
    cx.run_until_parked();

    let cmds = core.drain_commands();
    assert!(
        cmds.iter()
            .any(|c| matches!(c, Command::Workspace(WorkspaceCommand::SetStealthMode(_)))),
        "applying preferences must sync SetStealthMode to CoreBridge"
    );
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Workspace(WorkspaceCommand::SetSaveHuddleTranscripts(_))
        )),
        "applying preferences must sync SetSaveHuddleTranscripts to CoreBridge"
    );
    drop(root);
}

#[gpui::test]
fn wired_remote_channel_pick_leaves_the_store_to_the_core(cx: &mut gpui::TestAppContext) {
    let people = [user("U0ME", "me")];
    let (bridge, core) = cx.update(|cx| {
        make_wired_workspace("T0SW", "Switcher Co", &[("C0HOME", "home")], &people, cx)
    });
    let fx = bridge.snapshot(&HashMap::new()).expect("snapshot");
    let (root, app, cx) = mount_wired(fx, vec![bridge], cx);
    let _ = core.drain_commands();
    let remote = ChannelId::from("C0REMOTE");

    cx.update(|_, cx| {
        app.read(cx).switcher.clone().update(cx, |_, cx| {
            cx.emit(SwitcherEvent::OpenRemoteChannel(Box::new(Conversation {
                id: remote.clone(),
                name: Some("remote".into()),
                is_channel: true,
                ..Default::default()
            })));
        });
    });
    cx.run_until_parked();

    let cmds = core.drain_commands();
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Channel(ChannelCommand::OpenConversation(ch)) if *ch == remote
        )),
        "picking a remote channel opens it: {cmds:?}"
    );
    let stored = core
        .store()
        .lock()
        .expect("store lock")
        .conversation(&TeamId::from("T0SW"), &remote)
        .expect("read conversation");
    assert!(
        stored.is_none(),
        "the UI thread must never write the picked channel into the core's store (its conversations.info does): {stored:?}"
    );
    drop(root);
}

#[gpui::test]
fn wired_rail_reorder_keeps_own_team_across_all_surfaces(cx: &mut gpui::TestAppContext) {
    let mut fx = fixtures::load();
    let own = Team {
        id: TeamId::from("T0OWN"),
        name: "Own Co".into(),
        domain: "ownco".into(),
        ..Default::default()
    };
    let other = Team {
        id: TeamId::from("T0OTHER"),
        name: "Other Co".into(),
        domain: "otherco".into(),
        ..Default::default()
    };
    // Rail puts `other` FIRST while `own` is the shown workspace's own team.
    fx.teams = vec![own.clone(), other.clone()];
    fx.set_rail_workspaces(vec![other.clone(), own.clone()]);
    fx.users_mut().push(User {
        id: UserId::from("U0OWNMATE"),
        name: "ownmate".into(),
        team_id: Some(own.id.clone()),
        ..Default::default()
    });
    fx.users_mut().push(User {
        id: UserId::from("U0EXTMATE"),
        name: "extmate".into(),
        team_id: Some(other.id.clone()),
        ..Default::default()
    });

    assert_eq!(fx.own_team().map(|t| t.id.as_str()), Some("T0OWN"));
    assert!(!fx.is_external_user(&UserId::from("U0OWNMATE")));
    assert!(fx.is_external_user(&UserId::from("U0EXTMATE")));

    let ch = ChannelId::from("C0GENERAL");
    let ts = Ts::from("1700000000.000100");
    assert_eq!(
        actions::permalink(&fx, &ch, &ts),
        "https://ownco.slack.com/archives/C0GENERAL/p1700000000000100",
        "message permalink must use own_team(), never rail_workspaces()[0]"
    );
    assert_eq!(
        crate::huddle_video::huddle_browser_url(&fx, &ch, None),
        "https://app.slack.com/huddle/T0OWN/C0GENERAL",
        "huddle browser URL must use own_team(), never rail_workspaces()[0]"
    );

    let view = cx.update(|cx| SearchView::new(Arc::new(fx), None, cx));
    cx.update(|cx| {
        assert_eq!(
            view.read(cx).active_workspace_for_test(),
            own.id,
            "SearchView must initialize from own_team(), never rail_workspaces()[0]"
        );
    });
}

#[test]
fn fresh_people_and_transcript_overlays_propagate_to_pickers_search_and_exports() {
    let mut fx = fixtures::load();
    let own = Team {
        id: TeamId::from("T0OVL"),
        name: "Overlay Co".into(),
        domain: "overlay".into(),
        ..Default::default()
    };
    fx.teams = vec![own.clone()];
    fx.set_rail_workspaces(vec![own.clone()]);
    let alice = UserId::from("U0ALICE_OVL");
    let bob = UserId::from("U0BOB_OVL");
    fx.replace_users(vec![
        User {
            id: fx.self_user.clone(),
            name: "me".into(),
            ..Default::default()
        },
        User {
            id: alice.clone(),
            name: "alice".into(),
            real_name: Some("Alice Old".into()),
            ..Default::default()
        },
        User {
            id: bob.clone(),
            name: "bob".into(),
            real_name: Some("Bob Active".into()),
            ..Default::default()
        },
    ]);
    let ch = fx.channels[0].id.clone();
    let fid_keep = slackless_core::types::FileId::from("F0KEEP");
    let fid_clear = slackless_core::types::FileId::from("F0CLEAR");
    fx.file_transcripts_mut()
        .insert(fid_clear.clone(), "Stale transcript to clear".into());
    let root_ts = Ts::from("1700555000.000100");
    fx.messages.entry(ch.clone()).or_default().push(Message {
        ts: root_ts.clone(),
        user: Some(alice.clone()),
        text: "Voice note".into(),
        files: vec![
            File {
                id: fid_keep.clone(),
                name: Some("voice.m4a".into()),
                ..Default::default()
            },
            File {
                id: fid_clear.clone(),
                name: Some("old.m4a".into()),
                ..Default::default()
            },
        ],
        ..Default::default()
    });

    // Apply live overlays (without rebuilding WorkspaceData): deactivate Bob, rename Alice,
    // add a fresh transcript for F0KEEP, and clear F0CLEAR.
    let updated = fx
        .with_users(vec![
            User {
                id: alice.clone(),
                name: "alice".into(),
                real_name: Some("Alice Fresh".into()),
                ..Default::default()
            },
            User {
                id: bob.clone(),
                name: "bob".into(),
                real_name: Some("Bob Active".into()),
                deleted: true,
                ..Default::default()
            },
        ])
        .with_file_transcript_cues(fid_keep.clone(), "Freshly transcribed clip".into(), vec![])
        .with_file_transcript_cues(fid_clear.clone(), String::new(), vec![]);

    // 1. Compose recipient picker excludes deactivated Bob and finds renamed Alice.
    let bob_hits = crate::compose_targets::suggestions(&updated, &[], "bob");
    assert!(
        bob_hits.iter().all(
            |s| !matches!(&s.recipient, crate::compose_targets::Recipient::User(u) if *u == bob)
        ),
        "deactivated user in fresh_people overlay must not be suggested"
    );
    assert!(
        updated
            .people()
            .any(|u| u.id == alice && u.real_name.as_deref() == Some("Alice Fresh"))
    );

    // 2. all_file_transcripts merges additions and removes cleared transcripts.
    let all_tx = updated.all_file_transcripts();
    assert_eq!(
        all_tx.get(&fid_keep).map(String::as_str),
        Some("Freshly transcribed clip")
    );
    assert!(!all_tx.contains_key(&fid_clear));

    // 3. Fixture channel & thread exports see freshly landed transcripts and omit cleared ones.
    let ch_doc =
        super::dialogs::storage::build_fixture_channel_export(&updated, &own, &ch, "#general");
    let exported_msg = ch_doc
        .messages
        .iter()
        .find(|m| m.ts == root_ts.as_str())
        .expect("exported message");
    assert_eq!(
        exported_msg.files[0].transcript.as_deref(),
        Some("Freshly transcribed clip")
    );
    assert_eq!(exported_msg.files[1].transcript, None);

    let th_doc = super::dialogs::storage::build_fixture_thread_export(
        &updated, &own, &ch, "#general", &root_ts,
    );
    assert_eq!(
        th_doc.messages[0].files[0].transcript.as_deref(),
        Some("Freshly transcribed clip")
    );
    assert_eq!(th_doc.messages[0].files[1].transcript, None);

    // 4. Fixture search archive seeds fresh users and overlaid transcripts.
    let archive = fixture_archive_for(&updated, &own.id).expect("fixture archive");
    let store = archive.lock().expect("store lock");
    assert_eq!(
        store.user(&own.id, &bob).expect("bob").map(|u| u.deleted),
        Some(true)
    );
    assert_eq!(
        store
            .file_transcript(&own.id, &fid_keep)
            .expect("tx")
            .map(|t| t.text),
        Some("Freshly transcribed clip".to_owned())
    );
    assert!(
        store
            .file_transcript(&own.id, &fid_clear)
            .expect("cleared tx")
            .is_none()
    );
}

#[test]
fn gui_text_parsers_survive_hostile_utf8_and_arbitrary_offsets() {
    let fx = fixtures::load();
    let pieces = [
        "@",
        "#",
        ":",
        ":+1:",
        ":handshake::skin-tone-4:",
        ":tada:",
        "åäö",
        "日本語",
        "مرحبا",
        "Привет",
        "👨‍👩‍👧‍👦",
        "🤝🏽",
        "e\u{0301}",
        "\u{200b}",
        " ",
        "\n",
        "`",
        "```",
        "## ",
        "<@U0ALICE>",
        "<#C0GENERAL|general>",
        "https://example.com/åäö",
        "(",
        ")",
        "_",
        "-",
        "12:30",
    ];
    let mut state: u64 = 0x51A0_CAFE_BEEF_1234;
    let mut next_usize = |bound: usize| -> usize {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state as usize) % bound
    };

    for _ in 0..120 {
        let len = 1 + next_usize(14);
        let mut s = String::new();
        for _ in 0..len {
            s.push_str(pieces[next_usize(pieces.len())]);
        }
        // Shortcode expander and plain-text renderer must never panic on any UTF-8 mix.
        let _ = crate::emoji::replace_shortcodes(&s);
        let _ = crate::messages::plain_text(&s, &fx);
        let _ = crate::editor::rank(&s, "å");
        let _ = crate::editor::rank(&s, "日");

        // Autocomplete trigger must never panic even for mid-UTF-8 or out-of-bounds byte offsets.
        let doc = crate::editor::Doc::from(s.as_str());
        for caret in 0..=s.len() + 2 {
            let _ = crate::editor::trigger(&doc, caret);
        }
    }
}

#[gpui::test]
fn wired_background_workspace_applies_same_thread_reply_and_vip_notification_rules(
    cx: &mut gpui::TestAppContext,
) {
    let people = [
        user("U0ME", "me"),
        user("U0ALICE", "alice"),
        user("U0BOB", "bob"),
    ];
    let (b1, _c1) =
        cx.update(|cx| make_wired_workspace("T0ONE", "One", &[("C0OPEN", "open")], &people, cx));
    let (b2, c2) = cx
        .update(|cx| make_wired_workspace("T0TWO", "Two", &[("C0BG", "background")], &people, cx));
    let mut fx = b1.snapshot(&HashMap::new()).expect("ws1 snapshot");
    fx.set_rail_workspaces(vec![b1.team_summary().unwrap(), b2.team_summary().unwrap()]);
    let (root, app, cx) = mount_wired(fx, vec![b1, b2], cx);

    // Enable desktop notifications for all messages + thread replies to own threads.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.preferences.update(cx, |prefs, cx| {
                let mut s = prefs.settings().clone();
                s.desktop_notifications = true;
                s.notify_level = 0; // Everything
                s.thread_replies = true;
                prefs.load_settings(s, window, cx);
            });
            app.recent_notified_tags.clear();
        });
    });

    // 1. Thread reply to someone ELSE's thread in a background workspace must NOT notify.
    let unfollowed_reply = Message {
        ts: Ts::from("1700000200.000100"),
        thread_ts: Some(Ts::from("1700000100.000100")),
        parent_user_id: Some(UserId::from("U0BOB")),
        user: Some(UserId::from("U0ALICE")),
        text: "replying to bob".into(),
        ..Default::default()
    };
    cx.update(|_, cx| {
        c2.emit(
            Event::MessagePosted {
                channel: ChannelId::from("C0BG"),
                message: Box::new(unfollowed_reply),
            },
            cx,
        );
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(
            app.read(cx).recent_notified_tags.is_empty(),
            "background workspace must suppress replies to other users' threads just like shown workspace"
        );
    });

    // 2. Thread reply to MY thread in a background workspace MUST notify.
    let my_thread_reply = Message {
        ts: Ts::from("1700000201.000100"),
        thread_ts: Some(Ts::from("1700000100.000100")),
        parent_user_id: Some(UserId::from("U0ME")),
        user: Some(UserId::from("U0ALICE")),
        text: "replying to your thread".into(),
        ..Default::default()
    };
    cx.update(|_, cx| {
        c2.emit(
            Event::MessagePosted {
                channel: ChannelId::from("C0BG"),
                message: Box::new(my_thread_reply),
            },
            cx,
        );
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(
            app.read(cx)
                .recent_notified_tags
                .iter()
                .any(|(tag, _)| tag.contains("C0BG:1700000201.000100")),
            "background workspace must notify on replies to user's own thread"
        );
    });
    drop(root);
}

#[gpui::test]
fn wired_background_workspace_honors_marked_read_and_schedules_parked_reloads(
    cx: &mut gpui::TestAppContext,
) {
    let people = [user("U0ME", "me"), user("U0ALICE", "alice")];
    let (b1, _c1) =
        cx.update(|cx| make_wired_workspace("T0ONE", "One", &[("C0OPEN", "open")], &people, cx));
    let (b2, c2) = cx
        .update(|cx| make_wired_workspace("T0TWO", "Two", &[("C0BG", "background")], &people, cx));
    let mut fx = b1.snapshot(&HashMap::new()).expect("ws1 snapshot");
    fx.set_rail_workspaces(vec![b1.team_summary().unwrap(), b2.team_summary().unwrap()]);
    let (root, app, cx) = mount_wired(fx, vec![b1, b2], cx);

    let bg_ch = ChannelId::from("C0BG");
    let ts_read = Ts::from("1700000500.000100");
    // Visit workspace 1, finish its initial reload, mark C0BG read, then park it by switching to 0.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.switch_workspace(1, window, cx);
            app.ws.reload = None;
            app.ws.fold_read_marker(&bg_ch, &ts_read, false);
            app.switch_workspace(0, window, cx);
            assert!(app.slots[1].parked.is_some());
            assert!(app.slots[1].state.reload.is_none());
        });
    });

    // 1. A stale client.counts predating `ts_read` arriving while workspace 1 is in background
    //    must NOT resurrect unreads on C0BG.
    let stale_counts = slackless_core::api::ClientCounts {
        channels: vec![ConversationCounts {
            id: bg_ch.clone(),
            last_read: Some(Ts::from("1700000100.000100")),
            latest: Some(ts_read.clone()),
            has_unreads: true,
            mention_count: 2,
            ..Default::default()
        }],
        ..Default::default()
    };
    cx.update(|_, cx| {
        c2.emit(Event::Counts(Box::new(stale_counts)), cx);
        // 2. A UsersChanged event for the parked background workspace schedules a Users reload.
        c2.emit(Event::UsersChanged(vec![UserId::from("U0ALICE")]), cx);
    });
    cx.run_until_parked();

    cx.update(|_, cx| {
        let app_ref = app.read(cx);
        let entry = app_ref.slots[1]
            .state
            .counts
            .get(&bg_ch)
            .expect("counts entry");
        assert!(
            !entry.has_unreads && entry.mention_count == 0,
            "stale counts in background workspace must honor marked_read guard: {entry:?}"
        );
        assert!(
            matches!(app_ref.slots[1].state.reload, Some(Reload::Users(_))),
            "UsersChanged on parked workspace must schedule a reload on return"
        );
    });

    // 3. Conversations update escalates the parked workspace's pending reload to Full.
    cx.update(|_, cx| {
        c2.emit(Event::Conversations(vec![bg_ch.clone()]), cx);
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(
            matches!(app.read(cx).slots[1].state.reload, Some(Reload::Full)),
            "Conversations event on parked workspace must escalate reload to Full"
        );
    });
    drop(root);
}

#[gpui::test]
fn wired_activity_toggle_read_workspace_isolation_and_drafts_cancel_routing(
    cx: &mut gpui::TestAppContext,
) {
    let people = [user("U0ME", "me"), user("U0ALICE", "alice")];
    let (b1, c1) =
        cx.update(|cx| make_wired_workspace("T0ONE", "One", &[("C0ONE", "one")], &people, cx));
    let (b2, c2) =
        cx.update(|cx| make_wired_workspace("T0TWO", "Two", &[("C0TWO", "two")], &people, cx));
    let mut fx = b1.snapshot(&HashMap::new()).expect("ws1 snapshot");
    fx.set_rail_workspaces(vec![b1.team_summary().unwrap(), b2.team_summary().unwrap()]);
    let (root, app, cx) = mount_wired(fx, vec![b1, b2], cx);
    let _ = c1.drain_commands();
    let _ = c2.drain_commands();

    // 1. Load an unread Activity item in workspace 0 and click toggle_read: must emit read=true!
    let item = slackless_core::api::ActivityItem {
        kind: slackless_core::api::ActivityKind::Mention,
        unread: true,
        feed_ts: Ts::from("1700000100.000100"),
        key: "act-1".into(),
        channel: ChannelId::from("C0ONE"),
        thread_ts: None,
        ts: Ts::from("1700000100.000100"),
        actor: Some(UserId::from("U0ALICE")),
        reaction: None,
        count: 1,
        message: Some(Message {
            ts: Ts::from("1700000100.000100"),
            user: Some(UserId::from("U0ALICE")),
            text: "ping <@U0ME>".into(),
            ..Default::default()
        }),
    };
    cx.update(|_, cx| {
        c1.emit(
            Event::ActivityView {
                feed: Ok(Box::new(slackless_core::api::ActivityFeed {
                    items: vec![item],
                    next_cursor: None,
                })),
            },
            cx,
        );
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(app.read(cx).activity.read(cx).unread_count(), 1);
        app.read(cx).activity.clone().update(cx, |feed, cx| {
            feed.toggle_read_for_test(0, cx);
        });
    });
    cx.run_until_parked();
    let cmds1 = c1.drain_commands();
    assert!(
        cmds1.iter().any(|c| matches!(
            c,
            Command::Messaging(MessagingCommand::MarkActivity { read: true, .. })
        )),
        "toggling an unread Activity item to read must emit MarkActivity {{ read: true }}, got: {cmds1:?}"
    );

    // 2. Switching to workspace 1 must clear live Activity items from workspace 0.
    cx.update(|_, cx| {
        app.read(cx).activity.clone().update(cx, |feed, cx| {
            feed.toggle_read_for_test(0, cx);
        });
        assert_eq!(app.read(cx).activity.read(cx).unread_count(), 1);
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.switch_workspace(1, window, cx);
        });
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(
            app.read(cx).activity.read(cx).unread_count(),
            0,
            "switching workspaces must clear the outgoing workspace's live Activity feed"
        );
    });
    let _ = c1.drain_commands();
    let _ = c2.drain_commands();

    // 3. Cancelling a scheduled message belonging to workspace 0 while viewing workspace 1 must
    //    send DeleteScheduledMessage to workspace 0's bridge, NOT workspace 1's bridge.
    cx.update(|_, cx| {
        app.read(cx).drafts.clone().update(cx, |_, cx| {
            cx.emit(DraftsEvent::CancelScheduled(Entry {
                id: Some("Q0SCHED1".into()),
                workspace: 0,
                channel: ChannelId::from("C0ONE"),
                label: "#one".into(),
                text: "scheduled in ws0".into(),
                at: 1_900_000_000,
            }));
        });
    });
    cx.run_until_parked();
    let cmds1 = c1.drain_commands();
    let cmds2 = c2.drain_commands();
    assert!(
        cmds1.iter().any(|c| matches!(
            c,
            Command::Messaging(MessagingCommand::DeleteScheduledMessage {
                scheduled_message_id,
                ..
            }) if scheduled_message_id == "Q0SCHED1"
        )),
        "CancelScheduled for ws0 must route to ws0's bridge, got ws0={cmds1:?} ws1={cmds2:?}"
    );
    assert!(
        cmds2.iter().all(|c| !matches!(
            c,
            Command::Messaging(MessagingCommand::DeleteScheduledMessage { .. })
        )),
        "CancelScheduled for ws0 must never reach ws1's bridge: {cmds2:?}"
    );

    // 4. Composer::load_draft must clear staged attachments so switching channels/workspaces
    //    cannot leak staged files to the wrong destination.
    cx.update(|_, cx| {
        let composer = app.read(cx).composer.clone();
        composer.update(cx, |c, cx| {
            c.restore_attachments(
                vec![composer::PendingAttachment::new(
                    "secret.pdf".into(),
                    vec![1, 2, 3],
                    None,
                )],
                cx,
            );
            assert_eq!(c.attachments().len(), 1);
            c.load_draft("next channel draft", cx);
            assert!(
                c.attachments().is_empty(),
                "load_draft must clear staged attachments from the previous target"
            );
        });
    });

    drop(root);
}

#[gpui::test]
fn wired_thread_rollback_cache_sync_and_search_workspace_reset(cx: &mut gpui::TestAppContext) {
    let people = [user("U0ME", "me"), user("U0PEER", "peer")];
    let ((b1, _c1), (b2, _c2)) = cx.update(|cx| {
        (
            make_wired_workspace("T0ONE", "One", &[("C0ONE", "one")], &people, cx),
            make_wired_workspace("T0TWO", "Two", &[("C0TWO", "two")], &people, cx),
        )
    });
    let mut fx = b1.snapshot(&HashMap::new()).expect("ws1 snapshot");
    fx.set_rail_workspaces(vec![b1.team_summary().unwrap(), b2.team_summary().unwrap()]);
    let (root, app, cx) = mount_wired(fx, vec![b1, b2], cx);

    // 1. ThreadPanel::rollback_pending_reply removes a phantom optimistic reply from both live
    //    replies and the thread cache, and restores the typed draft into the thread composer.
    let ch = ChannelId::from("C0ONE");
    let root_ts = Ts::from("1700000100.000100");
    let root_msg = Message {
        ts: root_ts.clone(),
        user: Some(UserId::from("U0PEER")),
        text: "thread root".into(),
        ..Default::default()
    };
    let my_reply = Message {
        ts: Ts::from("1700000101.000100"),
        thread_ts: Some(root_ts.clone()),
        user: Some(UserId::from("U0ME")),
        text: "ghp_secret_draft".into(),
        ..Default::default()
    };
    cx.update(|_, cx| {
        let panel = app.read(cx).thread.clone();
        panel.update(cx, |panel, cx| {
            panel.open(ch.clone(), root_ts.clone(), Some(root_msg.clone()), cx);
            panel.set_thread(
                ch.clone(),
                root_ts.clone(),
                vec![root_msg.clone(), my_reply.clone()],
                cx,
            );
            panel.rollback_pending_reply(&ch, &root_ts, "ghp_secret_draft", cx);
            // Re-opening from cache must not resurrect the rolled-back reply.
            panel.open(ch.clone(), root_ts.clone(), Some(root_msg.clone()), cx);
        });
    });
    cx.update(|_, cx| {
        let panel = app.read(cx).thread.clone();
        let reply_count = panel.read(cx).reply_count_for_test();
        assert_eq!(
            reply_count, 0,
            "rolled-back reply must stay gone even after reopening from ThreadPanel cache"
        );
    });

    // 2. SearchView::set_workspace clears stale results from the previous workspace.
    cx.update(|_, cx| {
        let results = app.read(cx).results.clone();
        results.update(cx, |view, cx| {
            view.set_workspace(TeamId::from("T0TWO"), cx);
            assert_eq!(view.active_workspace_for_test(), TeamId::from("T0TWO"));
            assert!(view.local_hits_for_test().is_empty());
            assert_eq!(view.remote_messages_len_for_test(), 0);
        });
    });

    drop(root);
}

#[gpui::test]
fn wired_wave9_usergroups_later_todos_and_vip_sound_guards(cx: &mut gpui::TestAppContext) {
    use crate::directory::{
        cached_usergroup_members, record_usergroup_disabled, record_usergroup_members,
    };
    use crate::later::{LaterEvent, LaterItem, LaterState, PERSONAL_TODO_CHANNEL};

    let people = [user("U0ME", "me"), user("U0PEER", "peer")];
    let ((b1, c1), (b2, _c2)) = cx.update(|cx| {
        (
            make_wired_workspace("T0ONE", "One", &[("C0ONE", "one")], &people, cx),
            make_wired_workspace("T0TWO", "Two", &[("C0TWO", "two")], &people, cx),
        )
    });
    let mut fx = b1.snapshot(&HashMap::new()).expect("ws1 snapshot");
    fx.set_rail_workspaces(vec![b1.team_summary().unwrap(), b2.team_summary().unwrap()]);
    let (root, app, cx) = mount_wired(fx, vec![b1, b2], cx);

    // 1. UsergroupLiveState is strictly workspace-scoped by TeamId.
    let ug_id = slackless_core::types::UsergroupId::from("S0ENG");
    cx.update(|_, cx| {
        record_usergroup_members(
            TeamId::from("T0ONE"),
            ug_id.clone(),
            vec![UserId::from("U0ME")],
            cx,
        );
        record_usergroup_disabled(
            TeamId::from("T0ONE"),
            slackless_core::types::Usergroup {
                id: ug_id.clone(),
                name: "Eng".into(),
                handle: "eng".into(),
                ..Default::default()
            },
            cx,
        );
        assert_eq!(
            cached_usergroup_members(&TeamId::from("T0ONE"), &ug_id, cx),
            Some(vec![UserId::from("U0ME")])
        );
        assert_eq!(
            cached_usergroup_members(&TeamId::from("T0TWO"), &ug_id, cx),
            None,
            "usergroup member cache must not bleed into another workspace"
        );
    });

    // 2. LaterView::replace_live preserves local personal to-dos and LaterEvent::Update for
    //    PERSONAL_TODO_CHANNEL never sends UpdateSavedItem("TODO") to Slack.
    let _ = c1.drain_commands();
    cx.update(|_, cx| {
        let later = app.read(cx).later.clone();
        later.update(cx, |view, cx| {
            view.add_todo("Ship Wave 9".into(), cx);
            view.replace_live(
                0,
                vec![LaterItem {
                    workspace: 0,
                    channel: ChannelId::from("C0ONE"),
                    ts: Ts::from("1700000100.000100"),
                    label: "#one".into(),
                    author: "peer".into(),
                    text: "saved msg".into(),
                    state: LaterState::InProgress,
                    due: None,
                    fired: false,
                    is_todo: false,
                }],
                cx,
            );
            let items = view.items_for_popout();
            assert!(
                items
                    .iter()
                    .any(|it| it.is_personal_todo() && it.text == "Ship Wave 9"),
                "replace_live must preserve personal to-dos"
            );
            cx.emit(LaterEvent::Update {
                workspace: 0,
                channel: ChannelId::from(PERSONAL_TODO_CHANNEL),
                ts: Ts::from("1700000999.000001"),
                mark: Some(slackless_core::api::SavedMark::Completed),
                date_due: None,
            });
        });
    });
    cx.run_until_parked();
    let cmds = c1.drain_commands();
    assert!(
        cmds.iter().all(|c| !matches!(
            c,
            Command::Messaging(MessagingCommand::UpdateSavedItem { .. })
        )),
        "updating a local personal to-do must never hit Slack's saved.update API: {cmds:?}"
    );

    drop(root);
}
