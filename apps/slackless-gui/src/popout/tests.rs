use std::sync::Mutex;

use gpui_kit::gpui::{TestAppContext, gpui};
use slackless_core::MessagingCommand;

use crate::actions::MessageAction;
use crate::emoji::EmojiChoice;

use super::*;

#[gpui::test]
fn popout_message_actions_mutate_state_and_forward_commands(cx: &mut TestAppContext) {
    let fixtures = Arc::new(crate::fixtures::load());
    let channel = ChannelId::from("C0GENERAL");
    let sent: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sent_cb = sent.clone();
    let send: CommandForward = Arc::new(move |cmd| {
        let tag = match cmd {
            Command::Messaging(MessagingCommand::AddReaction { name, .. }) => format!("add:{name}"),
            Command::Messaging(MessagingCommand::RemoveReaction { name, .. }) => {
                format!("rm:{name}")
            }
            Command::Messaging(MessagingCommand::EditMessage { content, .. }) => {
                format!("edit:{}", content.text)
            }
            Command::Messaging(MessagingCommand::SaveItem { .. }) => "save".to_owned(),
            Command::Messaging(MessagingCommand::DeleteSavedItem { .. }) => "unsave".to_owned(),
            Command::Messaging(MessagingCommand::SetPinned { pinned, .. }) => {
                format!("pin:{pinned}")
            }
            _ => "other".to_owned(),
        };
        sent_cb.lock().unwrap().push(tag);
    });
    type JumpRecord = (ChannelId, Option<Ts>, bool);
    let jumped: Arc<Mutex<Vec<JumpRecord>>> = Arc::new(Mutex::new(Vec::new()));
    let jumped_cb = jumped.clone();
    let on_jump: JumpHandler = Rc::new(move |ch, ts, is_thread, _| {
        jumped_cb.lock().unwrap().push((ch, ts, is_thread));
    });

    cx.update(gpui_kit::init);
    let later = cx.update(LaterView::new);
    let mut popout_slot: Option<Entity<PopoutWindow>> = None;
    let vcx = cx.add_window(|window, cx| {
        let list = MessageList::new(fixtures.clone(), channel.clone(), window, cx);
        let win = cx.new(|cx| {
            PopoutWindow::new(
                PopoutRequest {
                    kind: PopoutKind::Conversation {
                        channel: channel.clone(),
                        source: list.downgrade(),
                        on_jump,
                    },
                    title: "# general".to_owned(),
                    fixtures: fixtures.clone(),
                    store: None,
                    team: None,
                    send,
                    later_sync: Some((later.downgrade(), 0)),
                    display_id: None,
                },
                window,
                cx,
            )
        });
        popout_slot = Some(win.clone());
        gpui_kit::component::Root::new(win, window, cx)
    });
    let popout = popout_slot.unwrap();
    let mut vcx = gpui_kit::VisualTestContext::from_window(vcx.into(), cx);

    let first = vcx.read(|cx| popout.read(cx).messages[0].clone());
    let ts = first.ts.clone();
    vcx.update(|window, cx| {
        popout.update(cx, |win, cx| {
            win.handle_message_action(
                channel.clone(),
                first.clone(),
                MessageAction::React("eyes".to_owned()),
                window,
                cx,
            );
            win.handle_message_action(
                channel.clone(),
                first.clone(),
                MessageAction::ToggleReaction("eyes".to_owned()),
                window,
                cx,
            );
            // Pick reaction via in-window EmojiPicker
            win.handle_message_action(
                channel.clone(),
                first.clone(),
                MessageAction::PickReaction(point(px(200.), px(200.))),
                window,
                cx,
            );
            assert!(win.picker.is_some());
            win.pick_emoji(
                &EmojiChoice {
                    name: "rocket".to_owned(),
                    glyph: Some("🚀".to_owned()),
                },
                window,
                cx,
            );
            assert!(win.picker.is_none());
            // Inline edit inside the popped-out window
            win.handle_message_action(
                channel.clone(),
                first.clone(),
                MessageAction::Edit,
                window,
                cx,
            );
            let input = win.editing.as_ref().unwrap().2.clone();
            input.update(cx, |s, cx| {
                s.set_value("Updated inline".to_owned(), window, cx)
            });
            win.save_edit(cx);
            assert!(win.editing.is_none());
            assert_eq!(win.messages[0].text, "Updated inline");

            win.handle_message_action(
                channel.clone(),
                first.clone(),
                MessageAction::SaveForLater,
                window,
                cx,
            );
            assert!(win.saved.contains(&ts));
            win.handle_message_action(
                channel.clone(),
                first.clone(),
                MessageAction::SaveForLater,
                window,
                cx,
            );
            assert!(!win.saved.contains(&ts));
            win.handle_message_action(
                channel.clone(),
                first.clone(),
                MessageAction::TogglePin,
                window,
                cx,
            );
            win.handle_message_action(
                channel.clone(),
                first.clone(),
                MessageAction::ReplyInThread,
                window,
                cx,
            );
        });
    });

    assert_eq!(
        *sent.lock().unwrap(),
        vec![
            "add:eyes",
            "rm:eyes",
            "add:rocket",
            "edit:Updated inline",
            "save",
            "unsave",
            "pin:true"
        ]
    );
    assert_eq!(*jumped.lock().unwrap(), vec![(channel, Some(ts), true)]);
}

#[gpui::test]
fn open_popout_targets_the_requested_display(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let second_display = DisplayId::from(2u64);
    cx.simulate_display_added(second_display, None);

    cx.update(|cx| {
        let opts0 = popout_window_options("# general", false, Some(second_display), 0, cx);
        let opts1 = popout_window_options("# general", false, Some(second_display), 1, cx);
        assert_eq!(opts0.display_id, Some(second_display));
        assert_eq!(opts1.display_id, Some(second_display));
        let b0 = opts0.window_bounds.unwrap().get_bounds();
        let b1 = opts1.window_bounds.unwrap().get_bounds();
        assert_eq!(b1.origin.x - b0.origin.x, px(28.));
        assert_eq!(b1.origin.y - b0.origin.y, px(28.));

        let default_opts = popout_window_options("# general", false, None, 0, cx);
        assert_eq!(default_opts.display_id, None);
    });
}

#[gpui::test]
fn popout_emoji_picker_occludes_scroll_wheel_from_messages(cx: &mut TestAppContext) {
    let fixtures = Arc::new(crate::fixtures::load());
    let channel = ChannelId::from("C0GENERAL");
    let send: CommandForward = Arc::new(|_| {});
    let on_jump: JumpHandler = Rc::new(|_, _, _, _| {});

    cx.update(gpui_kit::init);
    let mut popout_slot: Option<Entity<PopoutWindow>> = None;
    let vcx = cx.add_window(|window, cx| {
        let list = MessageList::new(fixtures.clone(), channel.clone(), window, cx);
        let win = cx.new(|cx| {
            let mut w = PopoutWindow::new(
                PopoutRequest {
                    kind: PopoutKind::Conversation {
                        channel: channel.clone(),
                        source: list.downgrade(),
                        on_jump,
                    },
                    title: "# general".to_owned(),
                    fixtures: fixtures.clone(),
                    store: None,
                    team: None,
                    send,
                    later_sync: None,
                    display_id: None,
                },
                window,
                cx,
            );
            // Ensure the transcript is taller than the viewport so it is scrollable.
            let seed = w.messages.clone();
            for i in 0..8 {
                for m in &seed {
                    let mut dup = m.clone();
                    dup.ts = Ts::from(format!("17600{i}0000.{}", m.ts.as_str()));
                    w.messages.push(dup);
                }
            }
            w
        });
        popout_slot = Some(win.clone());
        gpui_kit::component::Root::new(win, window, cx)
    });
    let popout = popout_slot.unwrap();
    let mut vcx = gpui_kit::VisualTestContext::from_window(vcx.into(), cx);
    vcx.run_until_parked();

    // Open the emoji picker inside the popout window and settle layout.
    vcx.update(|window, cx| {
        popout.update(cx, |win, cx| {
            win.open_picker(
                PopoutPickerTarget::Composer,
                point(px(360.), px(420.)),
                window,
                cx,
            );
        });
    });
    vcx.run_until_parked();

    let offset_before = vcx.read(|cx| popout.read(cx).scroll.offset());
    // Scroll inside the emoji picker overlay; the underlying message list must not move.
    vcx.simulate_event(gpui::ScrollEvent {
        position: point(px(260.), px(260.)),
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(120.))),
        touch_phase: gpui::TouchPhase::Moved,
        ..Default::default()
    });
    vcx.run_until_parked();
    let offset_after = vcx.read(|cx| popout.read(cx).scroll.offset());
    assert_eq!(
        offset_before, offset_after,
        "scrolling over the popout emoji picker must not scroll the underlying messages"
    );
}

#[gpui::test]
fn huddle_popout_thread_starts_empty_filters_root_and_uses_composer(cx: &mut TestAppContext) {
    use slackless_core::huddle::{HuddlePhase, HuddleSessionSnapshot};
    use slackless_core::types::MessageSubtype;

    let mut fixtures = crate::fixtures::load();
    let channel = ChannelId::from("C0GENERAL");
    let root_ts = Ts::from("1795500000.000100");
    let reply_ts = Ts::from("1795500001.000200");
    fixtures.threads.insert(
        (channel.clone(), root_ts.clone()),
        vec![
            Message {
                ts: root_ts.clone(),
                subtype: Some(MessageSubtype::HuddleThread),
                text: "A huddle started".to_owned(),
                ..Default::default()
            },
            Message {
                ts: reply_ts.clone(),
                thread_ts: Some(root_ts.clone()),
                text: "Agenda link".to_owned(),
                ..Default::default()
            },
        ],
    );
    let fixtures = Arc::new(fixtures);

    type SentRecord = (ChannelId, Option<Ts>, String, bool);
    let sent: Arc<Mutex<Vec<SentRecord>>> = Arc::new(Mutex::new(Vec::new()));
    let sent_cb = sent.clone();
    let send: CommandForward = Arc::new(move |cmd| {
        if let Command::Messaging(MessagingCommand::SendMessage {
            channel,
            content,
            thread_ts,
            broadcast,
        }) = cmd
        {
            sent_cb
                .lock()
                .unwrap()
                .push((channel, thread_ts, content.text, broadcast));
        }
    });
    let on_jump: JumpHandler = Rc::new(|_, _, _, _| {});
    let on_open_canvas: CanvasOpenHandler = Rc::new(|_, _| {});

    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });
    let initial_snap = HuddleSessionSnapshot {
        phase: HuddlePhase::Connected,
        channel: channel.clone(),
        thread_root_ts: None,
        ..Default::default()
    };
    let dock = cx.update(|cx| {
        crate::huddle_state::HuddleState::new(
            fixtures.clone(),
            initial_snap.clone(),
            Vec::new(),
            cx,
        )
    });
    let mut popout_slot: Option<Entity<PopoutWindow>> = None;
    let win_handle = cx.add_window(|window, cx| {
        let win = cx.new(|cx| {
            PopoutWindow::new(
                PopoutRequest {
                    kind: PopoutKind::Huddle {
                        source: dock.downgrade(),
                        on_jump,
                        on_open_canvas,
                    },
                    title: "Huddle".to_owned(),
                    fixtures: fixtures.clone(),
                    store: None,
                    team: None,
                    send,
                    later_sync: None,
                    display_id: None,
                },
                window,
                cx,
            )
        });
        popout_slot = Some(win.clone());
        gpui_kit::component::Root::new(win, window, cx)
    });
    let popout = popout_slot.unwrap();
    let mut vcx = gpui_kit::VisualTestContext::from_window(win_handle.into(), cx);
    vcx.run_until_parked();

    // 1. Brand-new huddle with thread_root_ts == None must NOT leak channel messages!
    vcx.read(|cx| {
        let win = popout.read(cx);
        assert!(
            win.huddle_thread_messages(&initial_snap).is_empty(),
            "fresh huddle thread must start empty, never pulling channel history"
        );
    });

    // 2. Once thread_root_ts is set, only actual replies appear (root huddle_thread is filtered).
    let with_root = HuddleSessionSnapshot {
        thread_root_ts: Some(root_ts.clone()),
        ..initial_snap.clone()
    };
    vcx.update(|_, cx| {
        dock.update(cx, |d, cx| d.update_session(with_root.clone(), cx));
    });
    vcx.run_until_parked();
    vcx.read(|cx| {
        let msgs = popout.read(cx).huddle_thread_messages(&with_root);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].ts, reply_ts);
        assert_eq!(msgs[0].text, "Agenda link");
    });

    // 3. Submitting through PopoutWindow's real Composer sends into the huddle thread!
    vcx.update(|_, cx| {
        popout.update(cx, |win, cx| {
            let content = crate::composer::content_of("https://example.com", &win.fixtures);
            win.send_content(content, cx);
        });
    });
    {
        let recorded = sent.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, channel);
        assert_eq!(recorded[0].1, Some(root_ts.clone()));
        assert_eq!(recorded[0].2, "https://example.com");
    }

    // 4. Once Slack echoes the reply with server-normalized text and server `ts`,
    //    `huddle_thread_messages` must NOT show a duplicate.
    let now_ts = Ts::from(format!(
        "{}.123456",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    ));
    let echoed = Message {
        ts: now_ts,
        thread_ts: Some(root_ts.clone()),
        user: Some(fixtures.self_user.clone()),
        text: "<https://example.com>".to_owned(),
        ..Default::default()
    };
    let mut updated_fixtures = crate::fixtures::load();
    updated_fixtures.threads.insert(
        (channel.clone(), root_ts.clone()),
        vec![
            Message {
                ts: root_ts.clone(),
                subtype: Some(MessageSubtype::HuddleThread),
                text: "A huddle started".to_owned(),
                ..Default::default()
            },
            Message {
                ts: reply_ts,
                thread_ts: Some(root_ts.clone()),
                text: "Agenda link".to_owned(),
                ..Default::default()
            },
            echoed,
        ],
    );
    vcx.update(|_, cx| {
        popout.update(cx, |win, _| {
            win.fixtures = Arc::new(updated_fixtures);
            let msgs = win.huddle_thread_messages(&with_root);
            assert_eq!(
                msgs.len(),
                2,
                "echoed server reply must deduplicate against the optimistic local entry"
            );
        });
    });
}

#[gpui::test]
fn huddle_popout_thread_drawer_skips_store_queries_on_unrelated_huddle_notifies(
    cx: &mut TestAppContext,
) {
    use slackless_core::huddle::{HuddleAudioLevels, HuddlePhase, HuddleSessionSnapshot};

    let fixtures = Arc::new(crate::fixtures::load());
    let store: SharedStore = Arc::new(Mutex::new(
        slackless_core::store::Store::open_in_memory().unwrap(),
    ));
    let send: CommandForward = Arc::new(|_| {});
    let on_jump: JumpHandler = Rc::new(|_, _, _, _| {});
    let on_open_canvas: CanvasOpenHandler = Rc::new(|_, _| {});
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });
    let state = cx.update(|cx| {
        crate::huddle_state::HuddleState::new(
            fixtures.clone(),
            HuddleSessionSnapshot {
                phase: HuddlePhase::Connected,
                channel: ChannelId::from("C0GENERAL"),
                ..Default::default()
            },
            Vec::new(),
            cx,
        )
    });
    let mut popout_slot: Option<Entity<PopoutWindow>> = None;
    let win_handle = cx.add_window(|window, cx| {
        let win = cx.new(|cx| {
            PopoutWindow::new(
                PopoutRequest {
                    kind: PopoutKind::Huddle {
                        source: state.downgrade(),
                        on_jump,
                        on_open_canvas,
                    },
                    title: "Huddle".to_owned(),
                    fixtures: fixtures.clone(),
                    store: Some(store),
                    team: Some(TeamId::from("T1")),
                    send,
                    later_sync: None,
                    display_id: None,
                },
                window,
                cx,
            )
        });
        popout_slot = Some(win.clone());
        gpui_kit::component::Root::new(win, window, cx)
    });
    let popout = popout_slot.unwrap();
    let mut vcx = gpui_kit::VisualTestContext::from_window(win_handle.into(), cx);
    vcx.update(|_, cx| {
        popout.update(cx, |win, _| win.huddle_drawer = Some(HuddleDrawer::Thread));
    });
    let before = vcx.read(|cx| popout.read(cx).reload_gen);

    // Audio levels notify ~8x/s while people talk: neither they nor a mute may touch SQLite.
    vcx.update(|_, cx| {
        state.update(cx, |s, cx| {
            let levels = HuddleAudioLevels {
                local_rms: 0.5,
                ..Default::default()
            };
            s.update_levels(levels, cx);
        });
    });
    vcx.update(|_, cx| state.update(cx, |s, cx| s.set_muted_local(true, cx)));
    vcx.run_until_parked();
    assert_eq!(
        vcx.read(|cx| popout.read(cx).reload_gen),
        before,
        "unrelated huddle notifies must not re-query the thread from the store"
    );

    // A thread root showing up re-queries once.
    vcx.update(|_, cx| {
        state.update(cx, |s, cx| {
            s.apply_room_meta(None, None, Some(Ts::from("1700000001.000100")), cx);
        });
    });
    vcx.run_until_parked();
    assert_eq!(vcx.read(|cx| popout.read(cx).reload_gen), before + 1);
}

#[gpui::test]
fn popout_mark_unread_edit_mrkdwn_refresh_edits_and_readonly_skips_composer(
    cx: &mut TestAppContext,
) {
    let fixtures = Arc::new(crate::fixtures::load());
    let channel = ChannelId::from("C0GENERAL");
    let sent: Arc<Mutex<Vec<Command>>> = Arc::new(Mutex::new(Vec::new()));
    let sent_cb = sent.clone();
    let send: CommandForward = Arc::new(move |cmd| {
        sent_cb.lock().unwrap().push(cmd);
    });
    let on_jump: JumpHandler = Rc::new(|_, _, _, _| {});

    cx.update(gpui_kit::init);
    let later = cx.update(LaterView::new);
    let mut conv_slot: Option<Entity<PopoutWindow>> = None;
    let mut readonly_slot: Option<Entity<PopoutWindow>> = None;
    let vcx = cx.add_window(|window, cx| {
        let list = MessageList::new(fixtures.clone(), channel.clone(), window, cx);
        let conv = cx.new(|cx| {
            PopoutWindow::new(
                PopoutRequest {
                    kind: PopoutKind::Conversation {
                        channel: channel.clone(),
                        source: list.downgrade(),
                        on_jump: on_jump.clone(),
                    },
                    title: "# general".to_owned(),
                    fixtures: fixtures.clone(),
                    store: None,
                    team: None,
                    send: send.clone(),
                    later_sync: None,
                    display_id: None,
                },
                window,
                cx,
            )
        });
        let readonly = cx.new(|cx| {
            PopoutWindow::new(
                PopoutRequest {
                    kind: PopoutKind::Later {
                        source: later.downgrade(),
                        on_jump,
                    },
                    title: "Saved".to_owned(),
                    fixtures: fixtures.clone(),
                    store: None,
                    team: None,
                    send,
                    later_sync: None,
                    display_id: None,
                },
                window,
                cx,
            )
        });
        conv_slot = Some(conv.clone());
        readonly_slot = Some(readonly);
        gpui_kit::component::Root::new(conv, window, cx)
    });
    let conv = conv_slot.unwrap();
    let readonly = readonly_slot.unwrap();
    let mut vcx = gpui_kit::VisualTestContext::from_window(vcx.into(), cx);

    // 1. Read-only popouts do NOT allocate Composer or EmojiPicker.
    vcx.read(|cx| {
        let ro = readonly.read(cx);
        assert!(ro.composer.is_none());
        assert!(ro.emoji_picker.is_none());
        let co = conv.read(cx);
        assert!(co.composer.is_some());
        assert!(co.emoji_picker.is_some());
    });

    vcx.update(|window, cx| {
        conv.update(cx, |win, cx| {
            // 2. MarkUnread on the first message sends ts_just_before (strictly before clicked ts).
            let first = win.messages[0].clone();
            win.handle_message_action(
                channel.clone(),
                first.clone(),
                MessageAction::MarkUnread,
                window,
                cx,
            );
            // 3. start_edit preserves raw mrkdwn formatting & mentions rather than plain_text.
            let formatted = Message {
                ts: Ts::from("1700000999.000100"),
                text: "*bold* cc <@U0BOB>".to_owned(),
                ..Default::default()
            };
            win.start_edit(channel.clone(), &formatted, window, cx);
            let edit_val = win.editing.as_ref().unwrap().2.read(cx).value().to_string();
            assert_eq!(edit_val, "*bold* cc <@U0BOB>");
            win.editing = None;

            // 4. apply_refreshed_messages detects remote edits/reactions even when len & last ts match!
            let mut edited_batch = win.messages.clone();
            edited_batch[0].text = "Remotely edited!".to_owned();
            win.apply_refreshed_messages(edited_batch, cx);
            assert_eq!(win.messages[0].text, "Remotely edited!");

            // 5. send_content strips tracking params and converts emoticons.
            let content = crate::composer::content_of(
                "hi :) https://example.com/?utm_source=test&keep=1",
                &win.fixtures,
            );
            win.send_content(content, cx);
        });
    });

    let cmds = sent.lock().unwrap();
    match &cmds[0] {
        Command::Messaging(MessagingCommand::MarkRead { ts, .. }) => {
            let first_ts = vcx.read(|cx| conv.read(cx).messages[0].ts.clone());
            assert_eq!(*ts, crate::threads_cards::ts_just_before(&first_ts));
            assert_ne!(*ts, first_ts);
        }
        other => panic!("expected MarkRead for MarkUnread, got {other:?}"),
    }
    match cmds.last().unwrap() {
        Command::Messaging(MessagingCommand::SendMessage { content, .. }) => {
            assert!(content.text.contains(":slightly_smiling_face:"));
            assert!(!content.text.contains("utm_source"));
            assert!(content.text.contains("keep=1"));
        }
        other => panic!("expected SendMessage, got {other:?}"),
    }
}

#[gpui::test]
fn popout_huddle_renders_all_drawers_popovers_themes_and_floating_panel(cx: &mut TestAppContext) {
    use crate::huddle_state::HuddleState;
    use crate::popout::floating_controls::HuddleFloatingPanel;
    use crate::theme::{self, SlackTheme};
    use slackless_core::huddle::{
        HuddleAudioLevels, HuddleDataEvent, HuddleDrawMessage, HuddleDrawSegment, HuddlePhase,
        HuddleReactionKind, HuddleSessionSnapshot, VideoFrame, VideoFrameData, VideoTileKey,
    };

    cx.update(|cx| {
        cx.set_global(crate::zoom::Zoom::default());
        gpui_kit::init(cx);
        theme::init(cx);
    });
    let fixtures = Arc::new(crate::fixtures::load());
    let channel = fixtures.channels[0].id.clone();
    let peer = fixtures
        .users
        .iter()
        .find(|u| u.id != fixtures.self_user)
        .unwrap()
        .id
        .clone();

    let state = cx.update(|cx| {
        let snap = HuddleSessionSnapshot {
            phase: HuddlePhase::Connected,
            channel: channel.clone(),
            call_id: "R0POP".into(),
            attendee_id: "att-me".into(),
            topic: Some("Design Critique".into()),
            background_id: Some("meadow".into()),
            jukebox_song: Some("lofi".into()),
            captions_enabled: true,
            video: true,
            sharing: true,
            share_source_id: Some("monitor:1".into()),
            started_at_secs: 1_700_000_000,
            ..Default::default()
        };
        let entity = HuddleState::new(fixtures.clone(), snap, vec![peer.clone()], cx);
        entity.update(cx, |s, cx| {
            s.update_levels(
                HuddleAudioLevels {
                    local_rms: 0.35,
                    active_speakers: vec![(peer.clone(), 0.9)],
                    muted_peers: vec![],
                },
                cx,
            );
            s.update_video_frame(
                VideoTileKey::RemoteCamera(peer.clone()),
                &VideoFrame {
                    width: 4,
                    height: 4,
                    data: VideoFrameData::Bgra(Arc::new(vec![180u8; 64])),
                },
                cx,
            );
            s.apply_data_event(
                &HuddleDataEvent::Reaction {
                    user: peer.clone(),
                    emoji: "tada".into(),
                    kind: HuddleReactionKind::Standard,
                    emoji_url: None,
                    at_ms: 0,
                },
                cx,
            );
            s.apply_data_event(
                &HuddleDataEvent::Caption {
                    user: peer.clone(),
                    text: "Looks great!".into(),
                    is_partial: false,
                    at_ms: 0,
                    lang: Some("en".into()),
                },
                cx,
            );
            s.apply_data_event(
                &HuddleDataEvent::Draw {
                    user: peer.clone(),
                    message: HuddleDrawMessage::New {
                        item_id: "d1".into(),
                        segment: HuddleDrawSegment {
                            a: [0.1, 0.1],
                            b: [0.2, 0.3],
                            c: [0.4, 0.5],
                            d: [0.6, 0.7],
                        },
                    },
                    at_ms: 0,
                },
                cx,
            );
        });
        entity
    });

    let sent: Arc<Mutex<Vec<Command>>> = Arc::new(Mutex::new(Vec::new()));
    let sent_cb = sent.clone();
    let send: CommandForward = Arc::new(move |cmd| {
        sent_cb.lock().unwrap().push(cmd);
    });
    let on_jump: JumpHandler = Rc::new(|_, _, _, _| {});
    let on_open_canvas: CanvasOpenHandler = Rc::new(|_, _| {});

    let mut popout_slot: Option<Entity<PopoutWindow>> = None;
    let state_in = state.clone();
    let fixtures_in = fixtures.clone();
    let vcx = cx.add_window(|window, cx| {
        let win = cx.new(|cx| {
            PopoutWindow::new(
                PopoutRequest {
                    kind: PopoutKind::Huddle {
                        source: state_in.downgrade(),
                        on_jump,
                        on_open_canvas,
                    },
                    title: "Huddle".to_owned(),
                    fixtures: fixtures_in,
                    store: None,
                    team: None,
                    send,
                    later_sync: None,
                    display_id: None,
                },
                window,
                cx,
            )
        });
        popout_slot = Some(win.clone());
        gpui_kit::component::Root::new(win, window, cx)
    });
    let popout = popout_slot.unwrap();
    let mut vcx = gpui_kit::VisualTestContext::from_window(vcx.into(), cx);

    // Cycle themes, backgrounds, ALL drawers (Thread/Notes/Diagnostics), and ALL popovers
    // (Invite/Devices/Camera/Share/Reactions/Theme/More/Music) while drawing every frame cleanly.
    for (mode, bg, drawer, popover) in [
        (SlackTheme::Dark, None, None, None),
        (
            SlackTheme::Light,
            Some("meadow"),
            Some(HuddleDrawer::Thread),
            Some(HuddlePopover::Reactions),
        ),
        (
            SlackTheme::Dark,
            Some("midnight"),
            Some(HuddleDrawer::Notes),
            Some(HuddlePopover::Theme),
        ),
        (
            SlackTheme::Light,
            Some("sky"),
            Some(HuddleDrawer::Diagnostics),
            Some(HuddlePopover::More),
        ),
        (
            SlackTheme::Dark,
            Some("forest"),
            None,
            Some(HuddlePopover::Share),
        ),
        (
            SlackTheme::Light,
            Some("sunset"),
            None,
            Some(HuddlePopover::Invite),
        ),
        (
            SlackTheme::Dark,
            Some("aurora"),
            None,
            Some(HuddlePopover::Devices),
        ),
        (
            SlackTheme::Light,
            Some("lavender"),
            None,
            Some(HuddlePopover::Camera),
        ),
        (SlackTheme::Dark, None, None, Some(HuddlePopover::Music)),
    ] {
        vcx.update(|window, cx| {
            theme::activate(mode, cx);
            state.update(cx, |s, cx| {
                s.set_background_local(bg.unwrap_or_default().to_owned(), cx);
            });
            popout.update(cx, |win, cx| {
                win.huddle_drawer = drawer;
                win.huddle_popover = popover;
                cx.notify();
            });
            let _ = window.draw(cx);
        });
    }

    // Also mount and draw `HuddleFloatingPanel` with draw mode on and off.
    let state_float = state.clone();
    let float_win = cx.add_window(|window, cx| {
        let panel = cx.new(|cx| HuddleFloatingPanel::new(state_float.downgrade(), window, cx));
        gpui_kit::component::Root::new(panel, window, cx)
    });
    let mut float_vcx = gpui_kit::VisualTestContext::from_window(float_win.into(), cx);
    float_vcx.update(|window, cx| {
        let _ = window.draw(cx);
        state.update(cx, |s, cx| s.set_draw_mode(true, cx));
        let _ = window.draw(cx);
        state.update(cx, |s, cx| s.set_draw_mode(false, cx));
        let _ = window.draw(cx);
    });
}
