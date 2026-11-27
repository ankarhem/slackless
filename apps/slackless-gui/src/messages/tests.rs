use super::render::*;
use super::rows::membership_run;
use gpui_kit::gpui::TextLayout;
use slackless_core::domain::mrkdwn::parse_mrkdwn_spans;
use std::cell::RefCell;

use gpui_kit::base::TextSelection;
use gpui_kit::gpui::{
    Entity, Font, Hsla, Modifiers, MouseButton, Render, TestAppContext, VisualTestContext, Window,
    point, px,
};

use super::*;

// The paragraphs recent frames painted, with their layouts, so tests can find where
// rendered text sits (gpui-base's own tests read this off its private window state).
thread_local! {
    static PAINTED: RefCell<Vec<(SharedString, TextLayout)>> = const { RefCell::new(Vec::new()) };
}

pub(super) fn record_painted_paragraph(text: &SharedString, layout: &TextLayout) {
    PAINTED.with(|painted| painted.borrow_mut().push((text.clone(), layout.clone())));
}

/// The middle of the character at `index` of the painted `text`.
fn painted_position(text: &str, index: usize, window: &Window, cx: &App) -> Point<Pixels> {
    let layout = PAINTED
        .with(|painted| {
            painted
                .borrow()
                .iter()
                .rev()
                .find(|(painted, _)| painted.as_ref() == text)
                .map(|(_, layout)| layout.clone())
        })
        .unwrap_or_else(|| panic!("{text:?} was not painted"));
    let position = layout
        .position_for_index(index)
        .unwrap_or_else(|| panic!("{text:?} has no character {index}"));
    let _ = (window, cx);
    point(position.x + px(1.), position.y + layout.line_height() / 2.)
}

fn selected_text(cx: &mut VisualTestContext) -> String {
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        TextSelection::selected_text(window, cx)
    })
}

fn press_at(
    text: &str,
    index: usize,
    click_count: usize,
    cx: &mut VisualTestContext,
) -> Point<Pixels> {
    let position = cx.update(|window, cx| painted_position(text, index, window, cx));
    cx.simulate_event(gpui_kit::gpui::MouseDownEvent {
        position,
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count,
        first_mouse: false,
    });
    position
}

fn drag_to(text: &str, index: usize, cx: &mut VisualTestContext) -> Point<Pixels> {
    let position = cx.update(|window, cx| painted_position(text, index, window, cx));
    cx.simulate_event(gpui_kit::gpui::MouseMoveEvent {
        position,
        pressed_button: Some(MouseButton::Left),
        modifiers: Modifiers::default(),
    });
    position
}

fn release_at(position: Point<Pixels>, click_count: usize, cx: &mut VisualTestContext) {
    cx.simulate_event(gpui_kit::gpui::MouseUpEvent {
        position,
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count,
    });
}

/// A transcript of one channel, mounted like the app does (the kit's root mounts the
/// selection layer; the harness stands in for it), recording routed links.
struct TranscriptHarness {
    links: Rc<RefCell<Vec<LinkTarget>>>,
    events: Rc<RefCell<Vec<&'static str>>>,
    list: Entity<MessageList>,
    _subscription: Subscription,
}

impl TranscriptHarness {
    fn open(channel: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let fixtures = Arc::new(crate::fixtures::load());
        let list = MessageList::new(fixtures, ChannelId::from(channel), window, cx);
        let links = Rc::new(RefCell::new(Vec::new()));
        let events = Rc::new(RefCell::new(Vec::new()));
        let subscription = {
            let links = links.clone();
            let events = events.clone();
            cx.subscribe(&list, move |_, _, event: &MessageListEvent, _| {
                if let MessageListEvent::OpenLink(target) = event {
                    links.borrow_mut().push(target.clone());
                }
                events.borrow_mut().push(match event {
                    MessageListEvent::OpenThread { .. } => "open-thread",
                    MessageListEvent::Changed { change, .. } => match change {
                        MessageChange::Pin { .. } => "pin-changed",
                        _ => "changed",
                    },
                    MessageListEvent::Later(_) => "later",
                    MessageListEvent::PickReaction { .. } => "pick-reaction",
                    _ => "event",
                });
            })
        };
        Self {
            links,
            events,
            list,
            _subscription: subscription,
        }
    }
}

impl Render for TranscriptHarness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(gpui_kit::base::TextSelectionLayer)
            .child(self.list.clone())
    }
}

/// The channel's transcript window, drawn once, then `run` with its harness and test
/// context (`add_window_view` lends the context only for the borrow). The window's
/// root is the kit's base [`gpui_kit::base::Root`], exactly as `open_window` mounts it,
/// so the toast layer behind [`crate::actions::notify`] exists.
fn with_transcript<R>(
    channel: &str,
    cx: &mut TestAppContext,
    run: impl FnOnce(Entity<TranscriptHarness>, &mut VisualTestContext) -> R,
) -> R {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });
    let harness = Rc::new(RefCell::new(None));
    let (root, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| TranscriptHarness::open(channel, window, cx));
        *harness.borrow_mut() = Some(view.clone());
        gpui_kit::base::Root::new(view, window, cx)
    });
    let harness = harness.borrow_mut().take().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        let _ = window.draw(cx);
    });
    let result = run(harness, cx);
    drop(root);
    result
}

#[test]
fn message_order_separates_messages_and_their_paragraphs() {
    let first = message_order(&Ts::new("1000000.001100"));
    let second = message_order(&Ts::new("1000000.002200"));
    assert!(first < second);
    // A message owns room for several blocks' worth of paragraphs.
    assert!(first + THEME_ORDER_STRIDE * 4 < second);
}

/// A run of joins (or leaves) ends at midnight, so the next day's first join opens its own
/// line under the date divider instead of being counted into the day before's.
#[test]
fn membership_runs_end_at_day_boundaries() {
    let membership = |subtype: &MessageSubtype, ts: &str| Message {
        subtype: Some(subtype.clone()),
        ts: Ts::from(ts),
        ..Default::default()
    };
    let (join, leave) = (MessageSubtype::ChannelJoin, MessageSubtype::ChannelLeave);
    assert!(membership_run(
        &membership(&join, "1000.000001"),
        &membership(&join, "1000.000002")
    ));
    assert!(
        !membership_run(
            &membership(&join, "1000.000001"),
            &membership(&join, "173800.000001")
        ),
        "a run does not cross into the next day"
    );
    assert!(
        !membership_run(
            &membership(&join, "1000.000001"),
            &membership(&leave, "1000.000002")
        ),
        "joins and leaves are separate runs"
    );
}

/// A run of joins reads as its first line: the joins folded into it keep their rows (and
/// scroll positions) but take no space, so a run of three sits as tight as a single join.
#[gpui::test]
fn folded_join_rows_take_no_space(cx: &mut TestAppContext) {
    let post = |ts: &str, user: &str, text: &str| Message {
        ts: Ts::from(ts),
        user: Some(UserId::from(user)),
        text: text.into(),
        ..Default::default()
    };
    let join = |ts: &str, user: &str| Message {
        subtype: Some(MessageSubtype::ChannelJoin),
        ..post(ts, user, "has joined the channel")
    };
    with_transcript("C0RANDOM", cx, |harness, cx| {
        harness.update(cx, |harness, cx| {
            harness.list.update(cx, |list, cx| {
                for message in [
                    post("1900000000.000100", "U0BOB", "Above the long run."),
                    join("1900000000.000200", "U0J1"),
                    join("1900000000.000300", "U0J2"),
                    join("1900000000.000400", "U0J3"),
                    post("1900000000.000500", "U0CARA", "Below the long run."),
                    post("1900000000.000600", "U0BOB", "Above the short run."),
                    join("1900000000.000700", "U0J4"),
                    post("1900000000.000800", "U0CARA", "Below the short run."),
                ] {
                    list.append(ChannelId::from("C0RANDOM"), message, cx);
                }
            });
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            let _ = window.draw(cx);
        });
        let y = |text: &str, cx: &mut VisualTestContext| {
            cx.update(|window, cx| painted_position(text, 0, window, cx).y)
        };
        let long = y("Below the long run.", cx) - y("Above the long run.", cx);
        let short = y("Below the short run.", cx) - y("Above the short run.", cx);
        assert_eq!(long, short, "the run's two folded joins leave no gap");
    });
}

#[gpui::test]
fn drag_selects_across_paragraphs_and_messages(cx: &mut TestAppContext) {
    // The deploy report: a rich-text section followed by a code block, then the latency
    // message's own paragraph.
    let section = "Deploy of api v2.4 is green on staging: dashboard";
    let code = "cargo deploy --service api --env staging";
    let next = "I'm seeing elevated p99 latency on /search since the deploy. Thread for details.";
    with_transcript("C0ENG", cx, |harness, cx| {
        // The transcript opens tail-anchored on the huge release report; scroll onto
        // the deploy report the test drags across.
        harness.update(cx, |harness, cx| {
            harness
                .list
                .update(cx, |list, cx| list.jump(Jump::Beginning, cx));
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            let _ = window.draw(cx);
        });
        assert_eq!(selected_text(cx), "");
        assert!(!PAINTED.with(|painted| painted.borrow().is_empty()));

        // Within one message: from the section into its code block.
        press_at(section, 7, 1, cx);
        let end = drag_to(code, code.len() - 8, cx);
        release_at(end, 1, cx);
        assert_eq!(
            selected_text(cx),
            format!("{}\n{}", &section[7..], &code[..code.len() - 8]),
            "a drag joins the message's paragraphs with a newline"
        );

        // Across messages: from the code block into the next message's paragraph.
        press_at(code, 6, 1, cx);
        let end = drag_to(next, 12, cx);
        release_at(end, 1, cx);
        assert_eq!(
            selected_text(cx),
            format!("{}\n{}", &code[6..], &next[..12]),
            "a drag spans messages in reading order"
        );
        drop(harness);
    });
}

#[test]
fn mrkdwn_fences_split_pre_blocks_out_of_the_text() {
    // Sentry's alert section: the whole text is one fence.
    assert_eq!(
        mrkdwn_fences("```undefined is not an object```"),
        vec![(true, "undefined is not an object")]
    );
    assert_eq!(
        mrkdwn_fences("Run:\n```\ncargo test\n```\nthen ship"),
        vec![(false, "Run:"), (true, "cargo test"), (false, "then ship")]
    );
    assert_eq!(mrkdwn_fences("a ``` b"), vec![(false, "a ``` b")]);
}

#[gpui::test]
fn blank_lines_render_as_a_short_paragraph_break(cx: &mut TestAppContext) {
    // Slack's blank line is an 8px `c-mrkdwn__br` block, not an empty text line
    // (measured on the official client: line pitch ~22.5px, blank-line pitch ~30px,
    // CSS height 8px). The fixture's "\n\n" sits inside one run, so splitting at the
    // break must slice runs and keep styling on both sides.
    let first = "Rollout notes:";
    let second = "Stage one is done, stage two next.";
    with_transcript("C0ENG", cx, |_, _| {
        // The blank-line message is the channel's last, so the tail-anchored transcript
        // paints it without jumping.
        let (across_break, line_height) = PAINTED.with(|painted| {
            let painted = painted.borrow();
            let find = |text: &str| {
                painted
                    .iter()
                    .rev()
                    .find(|(painted, _)| painted.as_ref() == text)
                    .map(|(_, layout)| layout.clone())
                    .unwrap_or_else(|| panic!("{text:?} was not painted"))
            };
            let first = find(first);
            let second = find(second);
            let line_height = first.line_height();
            let across_break =
                second.position_for_index(0).unwrap().y - first.position_for_index(0).unwrap().y;
            (across_break, line_height)
        });
        assert_eq!(
            across_break - (line_height + px(8.)),
            px(0.),
            "a blank line is Slack's short 8px break, not an empty text line"
        );
    });
}

#[gpui::test]
fn double_click_selects_the_word_under_the_cursor(cx: &mut TestAppContext) {
    let line = "Count me in.";
    with_transcript("C0RANDOM", cx, |_, cx| {
        let word = line.find("me").expect("the word is in the line");
        let at = press_at(line, word + 1, 2, cx);
        release_at(at, 2, cx);
        assert_eq!(selected_text(cx), "me");
    });
}

#[gpui::test]
fn clicking_a_mention_routes_to_the_profile(cx: &mut TestAppContext) {
    let fixtures = crate::fixtures::load();
    let line = format!(
        "@{} anyone up for lunch at the ramen place?",
        fixtures.mention_name(&UserId::from("U0ME"))
    );
    with_transcript("C0RANDOM", cx, |harness, cx| {
        let at = press_at(&line, 1, 1, cx);
        release_at(at, 1, cx);
        assert_eq!(
            harness.update(cx, |harness, _| harness.links.borrow().clone()),
            vec![LinkTarget::Profile(UserId::from("U0ME"))],
            "the @mention chip opens that user's profile"
        );
        // The click that opened the link left no selection behind.
        assert_eq!(selected_text(cx), "");
    });
}

/// The transcript reads mrkdwn as Slack (and core's parser) does: a marker styles only when a
/// closing one ends a word later on the line, so unbalanced markers stay literal.
#[gpui::test]
fn unbalanced_markers_render_literally(cx: &mut TestAppContext) {
    let rows = [
        (
            "1900000000.000100",
            "*nix and *BSD systems",
            "*nix and *BSD systems",
        ),
        (
            "1900000000.000200",
            "set _foo to my_var",
            "set _foo to my_var",
        ),
        ("1900000000.000300", "a *bold* claim", "a bold claim"),
    ];
    with_transcript("C0RANDOM", cx, |harness, cx| {
        harness.update(cx, |harness, cx| {
            harness.list.update(cx, |list, cx| {
                for (ts, text, _) in rows {
                    let message = Message {
                        ts: Ts::from(ts),
                        user: Some(UserId::from("U0BOB")),
                        text: text.into(),
                        ..Default::default()
                    };
                    list.append(ChannelId::from("C0RANDOM"), message, cx);
                }
            });
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            let _ = window.draw(cx);
        });
        for (_, text, shown) in rows {
            let painted = PAINTED.with(|painted| {
                painted
                    .borrow()
                    .iter()
                    .any(|(painted, _)| painted.as_ref() == shown)
            });
            assert!(painted, "{text:?} paints as {shown:?}");
        }
    });
}

#[gpui::test]
fn a_drag_over_a_link_selects_without_activating_it(cx: &mut TestAppContext) {
    // "dashboard" links to the grafana URL; dragging across it must select its text
    // without routing the link.
    let section = "Deploy of api v2.4 is green on staging: dashboard";
    with_transcript("C0ENG", cx, |harness, cx| {
        harness.update(cx, |harness, cx| {
            harness
                .list
                .update(cx, |list, cx| list.jump(Jump::Beginning, cx));
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            let _ = window.draw(cx);
        });
        press_at(section, 40, 1, cx);
        let end = drag_to(section, section.len(), cx);
        release_at(end, 1, cx);
        assert_eq!(selected_text(cx), &section[40..]);
        assert_eq!(
            harness.update(cx, |harness, _| harness.links.borrow().clone()),
            Vec::<LinkTarget>::new(),
            "a drag that selects text does not open the link it crossed"
        );
    });
}

/// The ts of the first C0ENG message whose text contains `needle`.
fn message_ts(harness: &Entity<TranscriptHarness>, cx: &mut VisualTestContext, needle: &str) -> Ts {
    harness.update(cx, |harness, cx| {
        harness
            .list
            .read(cx)
            .data
            .read(cx)
            .messages
            .iter()
            .find(|message| message.text.contains(needle))
            .map(|message| message.ts.clone())
            .unwrap_or_else(|| panic!("no fixture message contains {needle:?}"))
    })
}

#[gpui::test]
fn one_key_actions_reach_the_shared_handler_and_respect_the_ownership_rule(
    cx: &mut TestAppContext,
) {
    with_transcript("C0ENG", cx, |harness, cx| {
        let (own, other) = harness.update(cx, |harness, cx| {
            let list = harness.list.read(cx);
            let own = list
                .data
                .read(cx)
                .messages
                .iter()
                .find(|message| message.text.contains("Merged the reconnect"))
                .map(|message| message.ts.clone())
                .unwrap();
            let other = list
                .data
                .read(cx)
                .messages
                .iter()
                .find(|message| message.text.starts_with("Deploy of"))
                .map(|message| message.ts.clone())
                .unwrap();
            (own, other)
        });

        cx.update(|window, cx| {
            let list = harness.read(cx).list.clone();
            list.update(cx, |list, cx| {
                // Without a focused message (the draw may have left the synthetic
                // pointer on some row) the shortcuts are inert.
                list.data.update(cx, |data, _| data.active = None);
                list.focus_action(MessageAction::Edit, window, cx);
                assert!(list.data.read(cx).editing.is_none());

                // Someone else's message: Edit is not offered, so E does nothing.
                list.set_focused(&other, cx);
                assert_eq!(list.focused(cx).as_ref(), Some(&other));
                list.focus_action(MessageAction::Edit, window, cx);
                assert!(list.data.read(cx).editing.is_none());

                // Your own message: E edits it in place, loading its rich_text into the
                // WYSIWYG edit composer, and Esc cancels.
                list.set_focused(&own, cx);
                list.focus_action(MessageAction::Edit, window, cx);
                assert_eq!(list.data.read(cx).editing.as_ref(), Some(&own));
                let loaded = list.data.read(cx).edit_composer.read(cx).text();
                assert!(
                    loaded.contains("Merged the reconnect"),
                    "edit composer loaded the message text: {loaded:?}"
                );
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            let _ = window.draw(cx);
        });
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|_, cx| {
            let list = harness.read(cx).list.read(cx);
            assert!(
                list.data.read(cx).editing.is_none(),
                "Esc cancels inline WYSIWYG editing"
            );
        });
    });
}

#[gpui::test]
fn one_key_reply_and_pin_emit_the_transcript_events(cx: &mut TestAppContext) {
    with_transcript("C0ENG", cx, |harness, cx| {
        let own = message_ts(&harness, cx, "Merged the reconnect");

        cx.update(|window, cx| {
            let list = harness.read(cx).list.clone();
            list.update(cx, |list, cx| {
                list.set_focused(&own, cx);
                list.focus_action(MessageAction::ReplyInThread, window, cx);
                list.focus_action(MessageAction::TogglePin, window, cx);

                // The pin landed on the message's `pinned_to`.
                let channel = list.channel.clone();
                let pinned = list
                    .messages
                    .get(&channel)
                    .unwrap()
                    .iter()
                    .find(|message| message.ts == own)
                    .map(|message| message.pinned_to.contains(&channel));
                assert_eq!(pinned, Some(true));
            });
        });
        assert_eq!(
            harness.update(cx, |harness, _| harness.events.borrow().clone()),
            ["open-thread", "pin-changed"],
        );

        // P again unpins, and the change is reported the same way.
        cx.update(|window, cx| {
            let list = harness.read(cx).list.clone();
            list.update(cx, |list, cx| {
                list.focus_action(MessageAction::TogglePin, window, cx);
                let channel = list.channel.clone();
                let pinned = list
                    .messages
                    .get(&channel)
                    .unwrap()
                    .iter()
                    .find(|message| message.ts == own)
                    .map(|message| message.pinned_to.contains(&channel));
                assert_eq!(pinned, Some(false));
            });
        });
        assert_eq!(
            harness.update(cx, |harness, _| harness.events.borrow().clone()),
            ["open-thread", "pin-changed", "pin-changed"],
        );
    });
}

/// Dismissing an "Only visible to you" message drops its row from the virtual list with it,
/// as a delete does, and reports nothing to the store.
#[gpui::test]
fn dismissing_an_ephemeral_message_drops_its_row_from_the_virtual_list(cx: &mut TestAppContext) {
    with_transcript("C0ENG", cx, |harness, cx| {
        cx.update(|window, cx| {
            let list = harness.read(cx).list.clone();
            list.update(cx, |list, cx| {
                let before = list.data.read(cx).messages.len();
                assert_eq!(list.scroller.read(cx).item_count(), before);
                let ts = list.data.read(cx).messages[0].ts.clone();
                list.run_action(&ts, MessageAction::DismissEphemeral, window, cx);
                assert_eq!(list.data.read(cx).messages.len(), before - 1);
                assert_eq!(
                    list.scroller.read(cx).item_count(),
                    before - 1,
                    "the dismissed message's row left the list with it"
                );
            });
        });
        assert!(
            !harness.update(cx, |harness, _| harness
                .events
                .borrow()
                .contains(&"changed")),
            "dismissing is local: nothing is reported to the store"
        );
    });
}

#[gpui::test]
fn initial_unread_scroll_fires_once_and_ignores_backfill_refreshes(cx: &mut TestAppContext) {
    with_transcript("D0ALICE", cx, |harness, cx| {
        cx.update(|_, cx| {
            let list = harness.read(cx).list.clone();
            let channel = ChannelId::from("D0ALICE");
            list.update(cx, |list, cx| {
                list.set_conversation(channel.clone(), cx);
                assert!(
                    list.scroll_to_unread_once(cx),
                    "first open of unread channel should jump once"
                );
                assert!(
                    !list.scroll_to_unread_once(cx),
                    "subsequent StoreChanged/backfill loads must not jump back to unread"
                );
                // Prepending older history via set_history must preserve the window (not reset
                // to tail or re-arm unread jump).
                let mut expanded = vec![
                    Message {
                        ts: Ts::from("1000.000001"),
                        text: "older 1".into(),
                        ..Default::default()
                    },
                    Message {
                        ts: Ts::from("1000.000002"),
                        text: "older 2".into(),
                        ..Default::default()
                    },
                ];
                let before_len = list.data.read(cx).messages.len();
                expanded.extend(
                    list.data
                        .read(cx)
                        .messages
                        .iter()
                        .map(|m| Message::clone(m)),
                );
                list.set_history(channel, expanded, cx);
                assert_eq!(list.data.read(cx).messages.len(), before_len + 2);
                assert!(!list.scroller.read(cx).is_following_tail());
                assert!(
                    !list.scroll_to_unread_once(cx),
                    "prepended backfill page must not re-arm unread jump"
                );
            });
        });
    });
}

#[gpui::test]
fn recent_channel_history_cache_avoids_blank_flash_on_switch(cx: &mut TestAppContext) {
    with_transcript("C0ENG", cx, |harness, cx| {
        cx.update(|_, cx| {
            let list = harness.read(cx).list.clone();
            let c1 = ChannelId::from("C_LIVE_1");
            let c2 = ChannelId::from("C_LIVE_2");
            list.update(cx, |list, cx| {
                // Simulate live mode (`sample = false`) so LRU eviction runs.
                let mut live = crate::fixtures::load();
                live.sample = false;
                list.set_fixtures(Arc::new(live), cx);

                let m1 = Message {
                    ts: Ts::from("1700000001.000100"),
                    text: "channel 1 msg".into(),
                    ..Default::default()
                };
                let m2 = Message {
                    ts: Ts::from("1700000002.000100"),
                    text: "channel 2 msg".into(),
                    ..Default::default()
                };
                list.set_conversation(c1.clone(), cx);
                list.set_history(c1.clone(), vec![m1.clone()], cx);
                list.set_conversation(c2.clone(), cx);
                list.set_history(c2.clone(), vec![m2.clone()], cx);

                // Switching back to c1 immediately populates data.messages on frame 0.
                list.set_conversation(c1.clone(), cx);
                assert_eq!(list.data.read(cx).messages.len(), 1);
                assert_eq!(list.data.read(cx).messages[0].ts, m1.ts);
                assert_eq!(
                    list.message_by_ts(&c2, &m2.ts, cx).map(|m| m.text),
                    Some("channel 2 msg".to_owned())
                );
            });
        });
    });
}

#[gpui::test]
fn history_refreshes_and_older_pages_keep_the_visible_order_and_share_messages(
    cx: &mut TestAppContext,
) {
    with_transcript("C0ENG", cx, |harness, cx| {
        cx.update(|_, cx| {
            let list = harness.read(cx).list.clone();
            let channel = ChannelId::from("C_LIVE_PAGES");
            let message = |secs: u32, text: &str| Message {
                ts: Ts::from(format!("17000000{secs:02}.000000")),
                text: text.into(),
                ..Default::default()
            };
            let hidden = |secs: u32| Message {
                hidden: true,
                ..message(secs, "metadata event")
            };
            let visible = |list: &MessageList, cx: &App| -> Vec<String> {
                let data = list.data.read(cx);
                data.messages.iter().map(|m| m.text.clone()).collect()
            };
            list.update(cx, |list, cx| {
                let mut live = crate::fixtures::load();
                live.sample = false;
                list.set_fixtures(Arc::new(live), cx);
                list.set_conversation(channel.clone(), cx);
                list.set_history(
                    channel.clone(),
                    vec![
                        message(20, "two"),
                        hidden(30),
                        message(40, "four"),
                        message(50, "five"),
                    ],
                    cx,
                );
                assert_eq!(visible(list, cx), ["two", "four", "five"]);

                // An out-of-order older page: loaded and repeated messages are skipped.
                list.prepend_history(
                    channel.clone(),
                    vec![
                        message(10, "one"),
                        message(0, "zero"),
                        message(20, "two again"),
                        message(10, "one again"),
                        hidden(5),
                    ],
                    cx,
                );
                assert_eq!(visible(list, cx), ["zero", "one", "two", "four", "five"]);

                // A page overlapping the loaded window interleaves without reordering it.
                list.prepend_history(
                    channel.clone(),
                    vec![message(60, "six"), message(35, "three and a half")],
                    cx,
                );
                assert_eq!(
                    visible(list, cx),
                    [
                        "zero",
                        "one",
                        "two",
                        "three and a half",
                        "four",
                        "five",
                        "six"
                    ]
                );

                // A refreshed newest window replaces the loaded messages from its oldest one on
                // and keeps the older pages; an unchanged message keeps its shared allocation.
                let five = list.data.read(cx).messages[5].clone();
                let window = || {
                    vec![
                        message(40, "four (edited)"),
                        message(50, "five"),
                        message(70, "seven"),
                    ]
                };
                list.set_history(channel.clone(), window(), cx);
                assert_eq!(
                    visible(list, cx),
                    [
                        "zero",
                        "one",
                        "two",
                        "three and a half",
                        "four (edited)",
                        "five",
                        "seven"
                    ]
                );
                assert!(Rc::ptr_eq(&list.data.read(cx).messages[5], &five));

                // Re-reading the same window is a no-op, and the transcript holds the very
                // messages the history cache holds rather than copies.
                let shown = list.data.read(cx).messages.clone();
                list.set_history(channel.clone(), window(), cx);
                let data = list.data.read(cx);
                assert_eq!(data.messages.len(), shown.len());
                assert!(
                    data.messages
                        .iter()
                        .zip(&shown)
                        .all(|(a, b)| Rc::ptr_eq(a, b))
                );
                let cached: Vec<_> = list.messages[&channel]
                    .iter()
                    .filter(|m| !m.hidden)
                    .collect();
                assert_eq!(cached.len(), data.messages.len());
                assert!(
                    cached
                        .iter()
                        .zip(&data.messages)
                        .all(|(a, b)| Rc::ptr_eq(a, b))
                );
            });
        });
    });
}

#[gpui::test]
fn triple_click_is_bounded_inside_inline_code_and_fenced_code_blocks(cx: &mut TestAppContext) {
    let inline_line = "Check order.customerType: \"Organization\" please";
    let fenced_code = "fn main() {\n    println!(\"hello\");\n}";
    let inline_msg = Message {
        ts: Ts::from("1700000010.000100"),
        user: Some(UserId::from("U0ALICE")),
        text: "Check `order.customerType: \"Organization\"` please".into(),
        ..Default::default()
    };
    let fenced_msg = Message {
        ts: Ts::from("1700000011.000100"),
        user: Some(UserId::from("U0ALICE")),
        text: format!("```\n{fenced_code}\n```"),
        ..Default::default()
    };
    with_transcript("C0ENG", cx, |harness, cx| {
        harness.update(cx, |harness, cx| {
            let channel = ChannelId::from("C0CODE_CLICK");
            harness.list.update(cx, |list, cx| {
                list.set_conversation(channel.clone(), cx);
                list.set_history(channel, vec![inline_msg, fenced_msg], cx);
            });
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            let _ = window.draw(cx);
        });

        // Double-click inside multi-word inline code selects only the word under it: the whole
        // dotted identifier, as UAX #29 (and Slack) read it, still inside the pill.
        let word_ix = inline_line.find("customerType").unwrap() + 2;
        let at = press_at(inline_line, word_ix, 2, cx);
        release_at(at, 2, cx);
        assert_eq!(selected_text(cx), "order.customerType");

        // Triple-click inside inline code selects ONLY the inline code span, not the whole line.
        let at = press_at(inline_line, word_ix, 3, cx);
        release_at(at, 3, cx);
        assert_eq!(selected_text(cx), "order.customerType: \"Organization\"");

        // Triple-click on normal prose on the same line still selects the entire line.
        let prose_ix = inline_line.find("please").unwrap() + 1;
        let at = press_at(inline_line, prose_ix, 3, cx);
        release_at(at, 3, cx);
        assert_eq!(selected_text(cx), inline_line);

        // Double-click inside a multi-line fenced code block selects that word.
        let println_ix = fenced_code.find("println").unwrap() + 2;
        let at = press_at(fenced_code, println_ix, 2, cx);
        release_at(at, 2, cx);
        assert_eq!(selected_text(cx), "println");

        // Triple-click on any line of a multi-line fenced code block selects the ENTIRE block.
        let at = press_at(fenced_code, println_ix, 3, cx);
        release_at(at, 3, cx);
        assert_eq!(selected_text(cx), fenced_code);

        // Clicking elsewhere clears the bounded local selection cleanly.
        let at = press_at(inline_line, 0, 1, cx);
        release_at(at, 1, cx);
        assert_eq!(selected_text(cx), "");
    });
}

#[gpui::test]
fn snippet_card_lines_are_selectable_without_line_numbers(cx: &mut TestAppContext) {
    let line1 = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>";
    let line3 = "<plist version=\"1.0\">";
    let line5 = "    <key>ANSIBlackColor</key>";
    with_transcript("C0DESIGN", cx, |harness, cx| {
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            let _ = window.draw(cx);
        });

        // Drag across lines 1..3 of the Chalk_Custom.terminal snippet: code is selected cleanly
        // with NO line numbers (`1`, `2`, `3`) mixed into the copied text.
        press_at(line1, 0, 1, cx);
        let end = drag_to(line3, line3.len(), cx);
        release_at(end, 1, cx);
        let dragged = selected_text(cx);
        assert!(
            dragged.starts_with(line1) && dragged.ends_with(line3),
            "drag across snippet lines should select code text cleanly, got: {dragged:?}"
        );
        assert!(
            !dragged.contains("\n2 ") && !dragged.contains("\n3 "),
            "line numbers must not be included in selected snippet text: {dragged:?}"
        );

        // Double-click selects a single identifier inside the snippet.
        let key_ix = line5.find("ANSIBlackColor").unwrap() + 3;
        let at = press_at(line5, key_ix, 2, cx);
        release_at(at, 2, cx);
        assert_eq!(selected_text(cx), "ANSIBlackColor");

        // Triple-click selects the entire snippet line.
        let at = press_at(line1, 5, 3, cx);
        release_at(at, 3, cx);
        assert_eq!(selected_text(cx), line1);

        drop(harness);
    });
}

#[gpui::test]
fn video_transcript_text_is_selectable(cx: &mut TestAppContext) {
    let transcript = "Shipping video transcripts with selectable text";
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });

    let file = slackless_core::types::File {
        id: slackless_core::types::FileId::from("F0VIDTR"),
        name: Some("walkthrough.mp4".into()),
        mimetype: Some("video/mp4".into()),
        filetype: Some("mp4".into()),
        original_w: Some(1280),
        original_h: Some(720),
        url_private: Some("https://files.slack.com/pri/walkthrough.mp4".into()),
        ..Default::default()
    };
    let msg = slackless_core::types::Message {
        ts: slackless_core::types::Ts::from("1700000000.000100"),
        files: vec![file.clone()],
        ..Default::default()
    };
    let fixtures =
        crate::fixtures::load().with_file_transcript(file.id.clone(), transcript.to_owned());

    struct Host {
        fixtures: crate::fixtures::Fixtures,
        msg: slackless_core::types::Message,
    }
    impl Render for Host {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            gpui_kit::component::v_flex()
                .size_full()
                .p_4()
                .children(crate::files::attachments(
                    &self.fixtures,
                    &self.msg,
                    None,
                    cx,
                ))
                .child(gpui_kit::base::TextSelectionLayer)
        }
    }

    let (_view, cx) = cx.add_window_view(|_, cx| {
        crate::files::set_clip_transcript_expanded(file.id.clone(), true, cx);
        Host { fixtures, msg }
    });
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        let _ = window.draw(cx);
    });

    // Double-click selects a word inside the expanded video transcript.
    let word_ix = transcript.find("selectable").unwrap() + 2;
    let at = press_at(transcript, word_ix, 2, cx);
    release_at(at, 2, cx);
    assert_eq!(selected_text(cx), "selectable");

    // Drag selects a range across the transcript text.
    let start = transcript.find("video").unwrap();
    let end_ix = transcript.find("with").unwrap() + "with".len();
    press_at(transcript, start, 1, cx);
    let end = drag_to(transcript, end_ix, cx);
    release_at(end, 1, cx);
    assert_eq!(selected_text(cx), "video transcripts with");
}

fn test_run_theme() -> RunTheme {
    let fg = Hsla::default();
    RunTheme {
        base_font: Font::default(),
        mono_font: Font::default(),
        code_font: Font::default(),
        foreground: fg,
        muted: fg,
        border: fg,
        link: fg,
        mention_bg: fg,
        link_wash: fg,
        code_bg: fg,
        code_fg: fg,
        selection: fg,
        order: None,
        links: None,
        underline_links: false,
        dark: true,
        paragraphs: std::cell::Cell::new(0),
        key: SharedString::from("unit"),
    }
}

#[test]
fn jumbomoji_elements_accepts_one_to_five_standard_and_custom_and_rejects_others() {
    let mut fixtures = crate::fixtures::load();
    fixtures.custom_emoji.insert(
        "partyparrot".into(),
        crate::fixtures::EmojiImage::Url("https://example.com/parrot.gif".into()),
    );

    // 1..=5 emojis (including custom and whitespace) succeed.
    assert_eq!(jumbomoji_elements(":smile:", &fixtures).unwrap().len(), 1);
    assert_eq!(
        jumbomoji_elements("  :smile: :partyparrot: :rocket:  ", &fixtures)
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        jumbomoji_elements(":smile::smile::smile::smile::smile:", &fixtures)
            .unwrap()
            .len(),
        5
    );
    // A skin tone is part of its emoji, not a second one.
    assert_eq!(
        jumbomoji_elements(":+1::skin-tone-2:", &fixtures)
            .unwrap()
            .len(),
        1
    );

    // >5 emojis, empty, mixed text, or unknown shortcodes return None.
    assert!(
        jumbomoji_elements(":smile: :smile: :smile: :smile: :smile: :smile:", &fixtures).is_none()
    );
    assert!(jumbomoji_elements("", &fixtures).is_none());
    assert!(jumbomoji_elements("   ", &fixtures).is_none());
    assert!(jumbomoji_elements("hi :smile:", &fixtures).is_none());
    assert!(jumbomoji_elements(":smile:!", &fixtures).is_none());
    assert!(jumbomoji_elements(":definitely_not_a_real_emoji_xyz:", &fixtures).is_none());
    assert!(jumbomoji_elements("::", &fixtures).is_none());
}

/// One modeled mrkdwn line, flattened for assertions: its text, its links as `(label,
/// target)`, and its runs as `(text, style)` with plain runs left out.
struct Flat {
    text: String,
    links: Vec<(String, LinkTarget)>,
    runs: Vec<(String, RunStyle)>,
}

fn flat(text: &str, opts: TextOpts) -> Flat {
    let fixtures = crate::fixtures::load();
    let segments = line_segments(&parse_mrkdwn_spans(text), &fixtures, opts, false);
    let [
        Segment::Text {
            pieces,
            lead,
            trail,
        },
    ] = segments.as_slice()
    else {
        panic!("one text segment")
    };
    assert!(!lead && !trail, "a lone paragraph keeps its edge breaks");
    let [piece] = pieces.as_slice() else {
        panic!("one blank-line-free piece")
    };
    let text = piece.text.to_string();
    let links = piece
        .links
        .iter()
        .map(|(range, target)| (text[range.clone()].to_owned(), target.clone()))
        .collect();
    let mut at = 0;
    let runs = piece
        .runs
        .iter()
        .filter_map(|&(len, style)| {
            let run = text[at..at + len].to_owned();
            at += len;
            (style != RunStyle::plain()).then_some((run, style))
        })
        .collect();
    assert_eq!(at, text.len(), "runs cover the text");
    Flat { text, links, runs }
}

fn plain() -> TextOpts {
    TextOpts {
        emoji_text: false,
        verbatim: false,
    }
}

fn code_style() -> RunStyle {
    RunStyle::plain().with_flags(RunFlags {
        code: true,
        ..DEFAULT_FLAGS
    })
}

/// A line's model, as the transcript paints it: references named through the workspace and
/// routing to their targets, links styled as Slack draws them, bare URLs auto-linked under
/// linkify's boundary rules (not in verbatim text or inline code), markers literal unless
/// they close, emoji as glyphs, custom images or literal text.
#[test]
fn mrkdwn_lines_model_references_links_styles_and_emoji() {
    let fixtures = crate::fixtures::load();
    let me = UserId::from("U0ME");
    let me_name = format!("@{}", fixtures.mention_name(&me));
    let line = flat(
        "<@U0ME> and <@U0NOPE|zoe> in <#C0ENG|eng> or <#C0ENG>, <!here>, <!subteam^S1|@eng>, <!date^1700000000^{date_short}|Nov 14>:",
        plain(),
    );
    assert_eq!(
        line.text,
        format!("{me_name} and @zoe in #eng or #engineering, @here, @eng, Nov 14:"),
        "a user the workspace does not know goes by the label the text carries"
    );
    let link = |label: &str, target: LinkTarget| (label.to_owned(), target);
    assert_eq!(
        line.links,
        [
            link(&me_name, LinkTarget::Profile(me.clone())),
            link("@zoe", LinkTarget::Profile(UserId::from("U0NOPE"))),
            link("#eng", LinkTarget::Channel(ChannelId::from("C0ENG"))),
            link(
                "#engineering",
                LinkTarget::Channel(ChannelId::from("C0ENG"))
            ),
        ]
    );
    assert_eq!(
        line.runs
            .iter()
            .map(|(text, style)| (text.as_str(), style.tone))
            .collect::<Vec<_>>(),
        [
            (me_name.as_str(), Tone::Mention),
            ("@zoe", Tone::Mention),
            ("#eng", Tone::Reference),
            ("#engineering", Tone::Reference),
            ("@here", Tone::Mention),
            ("@eng", Tone::Mention),
        ]
    );
    assert!(line.runs.iter().all(|(_, style)| style.flags.semibold));

    // Links: a label styled whole takes its style (core carries it), other labels stay as
    // written (backticks included), emoji in a label expand, and a link labelled with its own
    // address is a chip.
    let line = flat(
        "<https://example.com/a&amp;b|*Docs*> and <https://example.com/x|`code`> <mailto:alice@example.com|Email> <https://example.com|:bar_chart: Panel>",
        plain(),
    );
    assert_eq!(line.text, "Docs and `code` Email 📊 Panel");
    assert_eq!(
        line.links
            .iter()
            .map(|(label, _)| label.as_str())
            .collect::<Vec<_>>(),
        ["Docs", "`code`", "Email", "📊 Panel"]
    );
    assert_eq!(
        line.links[0].1,
        LinkTarget::Url(SharedString::from("https://example.com/a&b"))
    );
    assert_eq!(
        line.links[2].1,
        LinkTarget::Url(SharedString::from("mailto:alice@example.com"))
    );
    let link_style = |flags: RunFlags| {
        RunStyle {
            tone: Tone::Link,
            ..RunStyle::plain()
        }
        .with_flags(flags)
    };
    assert_eq!(
        line.runs[0].1,
        link_style(RunFlags {
            bold: true,
            ..DEFAULT_FLAGS
        })
    );
    assert_eq!(line.runs[1].1, link_style(DEFAULT_FLAGS));
    assert_eq!(line.runs[2].1, link_style(DEFAULT_FLAGS));
    assert_eq!(line.runs[3].1, link_style(DEFAULT_FLAGS));
    let segments = line_segments(
        &parse_mrkdwn_spans("see <https://grafana.example/d/1|grafana.example/d/1>"),
        &fixtures,
        plain(),
        false,
    );
    assert!(matches!(
        segments.as_slice(),
        [Segment::Text { .. }, Segment::Chip { label, icon: IconName::Link, .. }] if label == "grafana.example/d/1"
    ));

    // Bare `http(s)://` URLs auto-link under linkify's boundary rules: balanced brackets stay
    // in the URL, stray trailing punctuation does not, and nothing else links.
    for (text, urls) in [
        (
            "See https://example.com/a&amp;b, and http://test.dev).",
            vec!["https://example.com/a&b", "http://test.dev"],
        ),
        (
            "(https://example.com/a_(b))",
            vec!["https://example.com/a_(b)"],
        ),
        (
            "https://example.com/q?x=1.",
            vec!["https://example.com/q?x=1"],
        ),
        ("https://", vec![]),
        ("www.example.com, alice@example.com and file.rs", vec![]),
    ] {
        let line = flat(text, plain());
        let linked: Vec<&str> = line.links.iter().map(|(label, _)| label.as_str()).collect();
        assert_eq!(linked, urls, "{text}");
        assert_eq!(
            line.text,
            text.replace("&amp;", "&"),
            "entities decode, nothing else changes"
        );
    }
    // Verbatim text (Block Kit `verbatim: true`) and inline code skip auto-linking.
    assert!(
        flat(
            "visit https://example.com",
            TextOpts {
                emoji_text: false,
                verbatim: true,
            }
        )
        .links
        .is_empty()
    );
    let code = flat("`https://example.com`", plain());
    assert!(code.links.is_empty());
    assert_eq!(code.runs[0].1, code_style());

    // Markers style only when a closer ends a word later on the line, as in Slack; the rest
    // of the line stays literal, and inline code keeps its markers.
    for literal in ["*nix and *BSD systems", "set _foo to my_var"] {
        let line = flat(literal, plain());
        assert_eq!(line.text, literal);
        assert!(line.runs.is_empty(), "{literal:?} carries no style");
    }
    let line = flat("a *bold* _italic_ ~strike~ `*code*` b", plain());
    assert_eq!(line.text, "a bold italic strike *code* b");
    assert_eq!(
        line.runs
            .iter()
            .map(|(text, _)| text.as_str())
            .collect::<Vec<_>>(),
        ["bold", "italic", "strike", "*code*"]
    );
    let styled = |flags| RunStyle::plain().with_flags(flags);
    assert_eq!(
        line.runs[0].1,
        styled(RunFlags {
            bold: true,
            ..DEFAULT_FLAGS
        })
    );
    assert_eq!(
        line.runs[1].1,
        styled(RunFlags {
            italic: true,
            ..DEFAULT_FLAGS
        })
    );
    assert_eq!(
        line.runs[2].1,
        styled(RunFlags {
            strike: true,
            ..DEFAULT_FLAGS
        })
    );
    assert_eq!(line.runs[3].1, code_style());

    // Emoji: a glyph, a custom emoji's image, or the literal shortcode (always in emoji text mode).
    assert_eq!(flat(":rocket: ships", plain()).text, "🚀 ships");
    let image = line_segments(
        &parse_mrkdwn_spans("go :partyparrot: now"),
        &fixtures,
        plain(),
        false,
    );
    assert!(matches!(
        image.as_slice(),
        [
            Segment::Text { .. },
            Segment::Image(_),
            Segment::Text { .. }
        ]
    ));
    assert_eq!(
        flat(":definitely_not_a_real_emoji_xyz:", plain()).text,
        ":definitely_not_a_real_emoji_xyz:"
    );
    assert_eq!(
        flat(
            ":rocket:",
            TextOpts {
                emoji_text: true,
                verbatim: false,
            }
        )
        .text,
        ":rocket:"
    );

    // The edit marker rides the paragraph's end, muted.
    let edited = line_segments(&parse_mrkdwn_spans("done"), &fixtures, plain(), true);
    let [Segment::Text { pieces, .. }] = edited.as_slice() else {
        panic!("one text segment")
    };
    assert_eq!(pieces[0].text, "done (edited)");
    assert_eq!(
        pieces[0].runs,
        [
            (4, RunStyle::plain()),
            (
                9,
                RunStyle {
                    tone: Tone::Muted,
                    ..RunStyle::plain()
                }
            )
        ]
    );
}

#[test]
fn strip_quote_prefix_hex_color_huddle_duration_and_ts_just_before() {
    assert_eq!(strip_quote_prefix("> hello"), Some("hello"));
    assert_eq!(strip_quote_prefix(">hello"), Some("hello"));
    assert_eq!(strip_quote_prefix("&gt; hello"), Some("hello"));
    assert_eq!(strip_quote_prefix("&gt;hello"), Some("hello"));
    assert_eq!(strip_quote_prefix(">"), Some(""));
    assert_eq!(strip_quote_prefix("plain"), None);

    assert!(hex_color("#2BAC76").is_some());
    assert!(hex_color("2BAC76").is_some());
    assert!(hex_color("#fff").is_none());
    assert!(hex_color("#zzzzzz").is_none());

    assert_eq!(format_huddle_duration(0), "0s");
    assert_eq!(format_huddle_duration(16), "16s");
    assert_eq!(format_huddle_duration(60), "1m");
    assert_eq!(format_huddle_duration(724), "12m 4s");
    assert_eq!(format_huddle_duration(3600), "1h");
    assert_eq!(format_huddle_duration(3900), "1h 5m");

    // ts_just_before: microsecond decrement, second borrow, zero floor, malformed fallback.
    assert_eq!(
        ts_just_before(&Ts::from("1700000000.000100")).as_str(),
        "1700000000.000099"
    );
    assert_eq!(
        ts_just_before(&Ts::from("1700000000.000000")).as_str(),
        "1699999999.999999"
    );
    assert_eq!(ts_just_before(&Ts::from("0.000000")).as_str(), "0.000000");
    assert_eq!(ts_just_before(&Ts::from("malformed")).as_str(), "0.000000");
    assert_eq!(ts_just_before(&Ts::from("abc.def")).as_str(), "0.000000");
}

#[test]
fn classify_author_badge_distinguishes_human_bot_and_via_app() {
    use slackless_core::types::{BotId, BotProfile, File, FileId, Team, TeamId};

    let mut fixtures = crate::fixtures::load();
    let human_id = UserId::from("U0ALICE");
    let bot_user_id = UserId::from("U9APPBOT");
    let bot_user = slackless_core::types::User {
        id: bot_user_id.clone(),
        name: "deploybot".to_owned(),
        is_bot: true,
        ..Default::default()
    };
    fixtures.users_mut().push(bot_user);

    // 1. Plain human message -> None
    let human_msg = Message {
        user: Some(human_id.clone()),
        ..Default::default()
    };
    assert_eq!(
        classify_author_badge(&human_msg, &fixtures),
        AuthorBadge::None
    );

    // 2. Bot user message -> App
    let bot_msg = Message {
        user: Some(bot_user_id),
        ..Default::default()
    };
    assert_eq!(classify_author_badge(&bot_msg, &fixtures), AuthorBadge::App);

    // 3. MessageSubtype::BotMessage -> App
    let subtype_bot = Message {
        subtype: Some(MessageSubtype::BotMessage),
        ..Default::default()
    };
    assert_eq!(
        classify_author_badge(&subtype_bot, &fixtures),
        AuthorBadge::App
    );

    // 4. Human user posting via an app (e.g. /giphy, workflow) -> ViaApp(app name)
    let via_msg = Message {
        user: Some(human_id),
        bot_id: Some(BotId::from("B9GIPHY")),
        bot_profile: Some(BotProfile {
            id: BotId::from("B9GIPHY"),
            name: "Giphy".to_owned(),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(
        classify_author_badge(&via_msg, &fixtures),
        AuthorBadge::ViaApp("Giphy".to_owned())
    );

    // 5. External file sharing label (F-COLLAB-879): external uploader & ext-shared channel
    let ext_team_id = TeamId::from("T9EXTFILE");
    fixtures.teams.push(Team {
        id: ext_team_id.clone(),
        name: "Acme Partner".to_owned(),
        domain: "acmepartner".to_owned(),
        ..Default::default()
    });
    let ext_user_id = UserId::from("U9EXTFILE");
    let ext_user = slackless_core::types::User {
        id: ext_user_id.clone(),
        team_id: Some(ext_team_id),
        ..Default::default()
    };
    fixtures.users_mut().push(ext_user);

    let mut file_msg = Message {
        user: Some(ext_user_id),
        files: vec![File {
            id: FileId::from("F9EXT"),
            name: Some("contract.pdf".to_owned()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let ch = fixtures.channels[0].id.clone();
    assert_eq!(
        external_file_sharing_label(&fixtures, &ch, &file_msg).as_deref(),
        Some("External file · Acme Partner")
    );

    // Internal uploader in an externally shared channel -> "Shared with external organizations"
    let mut shared_ch = fixtures.channels[0].clone();
    shared_ch.id = ChannelId::from("C9EXTSHARED");
    shared_ch.is_ext_shared = true;
    fixtures.channels.push(shared_ch.clone());
    file_msg.user = Some(fixtures.self_user.clone());
    assert_eq!(
        external_file_sharing_label(&fixtures, &shared_ch.id, &file_msg).as_deref(),
        Some("Shared with external organizations")
    );
    // Internal uploader in an internal channel -> None
    assert_eq!(external_file_sharing_label(&fixtures, &ch, &file_msg), None);
}

#[test]
#[ignore = "benchmark; run with --release -- --ignored --nocapture"]
fn benchmark_highlight_mrkdwn_and_time_1000_messages() {
    use std::hint::black_box;
    use std::time::Instant;

    let fixtures = crate::fixtures::load();
    let theme = test_run_theme();
    let opts = TextOpts {
        emoji_text: false,
        verbatim: false,
    };
    let now = time::CivilTime::from_unix_with_offset(1_764_000_000, 0);
    let code_sample = "pub fn handle(req: &Request) -> Result<Response, Error> {\n\
                       \tlet id = req.param(\"id\")?.parse::<u64>()?;\n\
                       \t// fast path\n\
                       \tOk(Response::json(id + 42))\n\
                       }";
    let mrkdwn_sample = "Deploy *api v2.4* by <@U0ALICE> in <#C0ENG|engineering> <!here> — \
                         see <https://grafana.example/d/1|dashboard> and `p99=12ms`.\n\
                         ```rust\nlet ok = true;\n```\n\
                         Visit https://status.example/inc/42 for details.";

    let count = 1_000usize;
    let start = Instant::now();
    let mut total_tokens = 0usize;
    let mut total_chars = 0usize;
    let mut models = Vec::with_capacity(count);
    for i in 0..count {
        // 1. Syntax highlighting
        let lang = crate::syntax::Lang::detect(Some("rust"), None, code_sample);
        total_tokens += crate::syntax::highlight(lang, code_sample).len();

        // 2. The message's body model: fence splitting, spans, segments, code highlighting
        let message = Message {
            text: format!("{mrkdwn_sample} #{i}"),
            ..Default::default()
        };
        let model = BodyModel::new(&message, &fixtures, opts);
        let blocks = model.fallback(&fixtures, opts);
        for block in blocks.iter() {
            match block {
                MrkdwnBlock::Code(code) => total_tokens += code.tokens.len(),
                MrkdwnBlock::Line(segments) => {
                    for segment in segments {
                        if let Segment::Text { pieces, .. } = segment {
                            total_chars +=
                                pieces.iter().map(|piece| piece.text.len()).sum::<usize>();
                        }
                    }
                }
                _ => {}
            }
        }
        models.push(blocks);

        // 3. Time & date formatting
        let msg_time =
            time::CivilTime::from_unix_with_offset(1_764_000_000 - (i as i64) * 7_200, 0);
        total_chars += time::card_stamp_at(&msg_time, &now, i % 2 == 0).len();
        total_chars += time::list_stamp_at(&msg_time, &now).len();
        total_chars += time::format_clock_civil(&msg_time, false).len();
    }
    let cold = start.elapsed();
    // 4. Warm frames: 50 visible rows rendered from their cached models, elements only.
    let warm = Instant::now();
    for _ in 0..20 {
        for blocks in &models[..50] {
            theme.paragraphs.set(0);
            black_box(render_mrkdwn(black_box(blocks), &theme));
        }
    }
    let warm = warm.elapsed();
    let ms = |elapsed: std::time::Duration| elapsed.as_secs_f64() * 1_000.0;
    let per_sec = (count as f64) / cold.as_secs_f64().max(1e-9);
    eprintln!(
        "bench 1000 messages, cold model (syntax + mrkdwn + time): {:.2} ms ({per_sec:.0} msgs/s; tokens={total_tokens}, chars={total_chars}); warm 50 rows x 20 frames: {:.2} ms ({:.3} ms/frame)",
        ms(cold),
        ms(warm),
        ms(warm) / 20.
    );
    assert!(total_tokens > 0 && total_chars > 0);
}

#[test]
fn same_group_guards_membership_anonymous_bots_and_distinct_broadcasts() {
    use slackless_core::types::BotId;

    let mut base = Message {
        ts: Ts::from("1700000100.000100"),
        user: Some(UserId::from("U1")),
        text: "first".into(),
        ..Default::default()
    };
    let followup = Message {
        ts: Ts::from("1700000110.000100"),
        user: Some(UserId::from("U1")),
        text: "second".into(),
        ..Default::default()
    };
    assert!(same_group(&base, &followup));

    // Membership notice followed by a regular message from the same user must not group.
    base.subtype = Some(MessageSubtype::ChannelJoin);
    assert!(!same_group(&base, &followup));

    // Anonymous messages with neither user, bot_id, nor username must never group.
    let anon1 = Message {
        ts: Ts::from("1700000100.000100"),
        ..Default::default()
    };
    let anon2 = Message {
        ts: Ts::from("1700000105.000100"),
        ..Default::default()
    };
    assert!(!same_group(&anon1, &anon2));

    // Two distinct bot_ids without user/username must not group; same bot_id does.
    let bot_a = Message {
        ts: Ts::from("1700000100.000100"),
        bot_id: Some(BotId::from("B1")),
        ..Default::default()
    };
    let bot_b = Message {
        ts: Ts::from("1700000105.000100"),
        bot_id: Some(BotId::from("B2")),
        ..Default::default()
    };
    let bot_a2 = Message {
        ts: Ts::from("1700000108.000100"),
        bot_id: Some(BotId::from("B1")),
        ..Default::default()
    };
    assert!(!same_group(&bot_a, &bot_b));
    assert!(same_group(&bot_a, &bot_a2));

    // Two thread broadcasts from different threads must not group; same thread does.
    let bc1 = Message {
        ts: Ts::from("1700000100.000100"),
        user: Some(UserId::from("U1")),
        thread_ts: Some(Ts::from("1700000010.000100")),
        subtype: Some(MessageSubtype::ThreadBroadcast),
        ..Default::default()
    };
    let bc_other = Message {
        ts: Ts::from("1700000105.000100"),
        user: Some(UserId::from("U1")),
        thread_ts: Some(Ts::from("1700000020.000100")),
        subtype: Some(MessageSubtype::ThreadBroadcast),
        ..Default::default()
    };
    let bc_same = Message {
        ts: Ts::from("1700000108.000100"),
        user: Some(UserId::from("U1")),
        thread_ts: Some(Ts::from("1700000010.000100")),
        subtype: Some(MessageSubtype::ThreadBroadcast),
        ..Default::default()
    };
    assert!(!same_group(&bc1, &bc_other));
    assert!(same_group(&bc1, &bc_same));
}

#[gpui::test]
fn message_list_pin_refresh_edit_last_own_skips_ghost_and_jump_reveal_guards(
    cx: &mut TestAppContext,
) {
    use std::rc::Rc;

    cx.update(gpui_kit::init);
    let fixtures = Arc::new(crate::fixtures::load());
    let ch = fixtures.channels[0].id.clone();
    let me = fixtures.self_user.clone();

    let real_ts = Ts::from("1700000100.000100");
    let ghost_ts = Ts::from("1700000200.000100");
    let eph_ts = Ts::from("1700000300.000100");
    let newest_ts = Ts::from("1700000400.000100");

    let real_own = Message {
        ts: real_ts.clone(),
        user: Some(me.clone()),
        text: "editable message".into(),
        ..Default::default()
    };
    let mut ghost_own = Message {
        ts: ghost_ts.clone(),
        user: Some(me.clone()),
        text: "deleted".into(),
        ..Default::default()
    };
    ghost_own.extra.insert(
        crate::bridge::DELETED_EXTRA_KEY.to_owned(),
        serde_json::Value::Bool(true),
    );
    let eph_own = Message {
        ts: eph_ts.clone(),
        user: Some(me.clone()),
        text: "ephemeral".into(),
        is_ephemeral: true,
        ..Default::default()
    };
    let other_newest = Message {
        ts: newest_ts.clone(),
        user: Some(UserId::from("U0OTHER")),
        text: "newest".into(),
        ..Default::default()
    };

    let need_history_count = Rc::new(RefCell::new(0usize));
    let list_slot: Rc<RefCell<Option<Entity<MessageList>>>> = Rc::new(RefCell::new(None));

    struct Host {
        list: Entity<MessageList>,
        _sub: Subscription,
    }
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.list.clone()
        }
    }

    let nhc = need_history_count.clone();
    let slot = list_slot.clone();
    let fx = fixtures.clone();
    let ch_init = ch.clone();
    let (_root, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| {
            let list = MessageList::new(fx, ch_init, window, cx);
            *slot.borrow_mut() = Some(list.clone());
            let sub = cx.subscribe(&list, move |_, _, ev: &MessageListEvent, _| {
                if matches!(ev, MessageListEvent::NeedHistorySince { .. }) {
                    *nhc.borrow_mut() += 1;
                }
            });
            Host { list, _sub: sub }
        });
        gpui_kit::base::Root::new(view, window, cx)
    });
    let list = list_slot.borrow_mut().take().unwrap();
    cx.run_until_parked();

    cx.update(|window, cx| {
        list.update(cx, |l, cx| {
            l.set_history(
                ch.clone(),
                vec![
                    real_own.clone(),
                    ghost_own.clone(),
                    eph_own.clone(),
                    other_newest.clone(),
                ],
                cx,
            );

            // 1. TogglePin refreshes data.messages immediately.
            l.run_action(&real_ts, MessageAction::TogglePin, window, cx);
            let pinned_in_data = l
                .data
                .read(cx)
                .messages
                .iter()
                .find(|m| m.ts == real_ts)
                .is_some_and(|m| m.pinned_to.contains(&ch));
            assert!(
                pinned_in_data,
                "TogglePin must refresh MessageData snapshot immediately"
            );

            // 2. edit_last_own & latest_own_ts skip ephemeral and deleted ghost own messages.
            assert_eq!(l.latest_own_ts(cx), Some(real_ts.clone()));
            l.edit_last_own(window, cx);
            assert_eq!(l.data.read(cx).editing, Some(real_ts.clone()));

            // 3. reveal() with a ts NEWER than the loaded tail stays at newest and does not emit NeedHistorySince.
            l.reveal(&Ts::from("1700009999.000100"), cx);
            assert_eq!(*need_history_count.borrow(), 0);
            assert_eq!(l.data.read(cx).highlighted, Some(newest_ts.clone()));

            // 4. finish_jump() when the target ts was deleted highlights the landed message ts.
            l.pending_highlight = Some(Ts::from("1700000150.000100"));
            l.finish_jump(cx);
            assert_eq!(l.data.read(cx).highlighted, Some(ghost_ts.clone()));
        });
    });
}
