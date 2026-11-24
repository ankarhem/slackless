use std::sync::Mutex;

use gpui_kit::gpui::TestAppContext;

use super::*;

#[gpui::test]
fn popout_message_actions_mutate_state_and_forward_commands(cx: &mut TestAppContext) {
    let fixtures = Arc::new(crate::fixtures::load());
    let channel = ChannelId::from("C0GENERAL");
    let sent: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sent_cb = sent.clone();
    let send: CommandForward = Arc::new(move |cmd| {
        let tag = match cmd {
            Command::AddReaction { name, .. } => format!("add:{name}"),
            Command::RemoveReaction { name, .. } => format!("rm:{name}"),
            Command::EditMessage { content, .. } => format!("edit:{}", content.text),
            Command::SaveItem { .. } => "save".to_owned(),
            Command::DeleteSavedItem { .. } => "unsave".to_owned(),
            Command::SetPinned { pinned, .. } => format!("pin:{pinned}"),
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
                PopoutKind::Conversation {
                    channel: channel.clone(),
                    source: list.downgrade(),
                    on_jump,
                },
                "# general".to_owned(),
                fixtures.clone(),
                None,
                None,
                send,
                Some((later.downgrade(), 0)),
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
                PopoutKind::Conversation {
                    channel: channel.clone(),
                    source: list.downgrade(),
                    on_jump,
                },
                "# general".to_owned(),
                fixtures.clone(),
                None,
                None,
                send,
                None,
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
        if let Command::SendMessage {
            channel,
            content,
            thread_ts,
            broadcast,
        } = cmd
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
        crate::huddle_dock::HuddleDock::new(fixtures.clone(), initial_snap.clone(), Vec::new(), cx)
    });
    let mut popout_slot: Option<Entity<PopoutWindow>> = None;
    let win_handle = cx.add_window(|window, cx| {
        let win = cx.new(|cx| {
            PopoutWindow::new(
                PopoutKind::Huddle {
                    source: dock.downgrade(),
                    on_jump,
                    on_open_canvas,
                },
                "Huddle".to_owned(),
                fixtures.clone(),
                None,
                None,
                send,
                None,
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
            let content = crate::composer::content_of("Notes from call", &win.fixtures);
            win.send_content(content, cx);
        });
    });
    let recorded = sent.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].0, channel);
    assert_eq!(recorded[0].1, Some(root_ts));
    assert_eq!(recorded[0].2, "Notes from call");
}
