use super::render::*;
use super::rows::membership_run;
use gpui_kit::gpui::TextLayout;
use std::cell::RefCell;

use gpui_kit::base::TextSelection;
use gpui_kit::gpui::{
    Entity, Font, Modifiers, MouseButton, Render, TestAppContext, VisualTestContext, Window, point,
    px,
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

#[test]
fn link_labels_take_the_styles_wrapping_them() {
    let (label, run) = styled_label("*TypeError*", RunFlags::default());
    assert_eq!(label, "TypeError");
    assert!(run.bold && !run.italic);
    let (label, run) = styled_label("_*both*_", RunFlags::default());
    assert_eq!(label, "both");
    assert!(run.bold && run.italic);
    let (label, run) = styled_label("a *b*", RunFlags::default());
    assert_eq!(label, "a *b*");
    assert!(!run.bold);
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

        // Double-click inside multi-word inline code selects only that word.
        let word_ix = inline_line.find("customerType").unwrap() + 2;
        let at = press_at(inline_line, word_ix, 2, cx);
        release_at(at, 2, cx);
        assert_eq!(selected_text(cx), "customerType");

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

fn test_run_theme(verbatim: bool) -> RunTheme {
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
        emoji_text_mode: false,
        dark: true,
        verbatim,
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

#[test]
fn angle_token_parses_users_channels_specials_dates_links_and_mailto() {
    let fixtures = crate::fixtures::load();
    let theme = test_run_theme(false);

    // <@U0ME>
    let mut b = ParagraphBuilder::new();
    let consumed = angle_token(&mut b, "<@U0ME> rest", &fixtures, &theme, DEFAULT_FLAGS);
    assert_eq!(consumed, Some(7));
    assert_eq!(
        b.text,
        format!("@{}", fixtures.mention_name(&UserId::from("U0ME")))
    );
    assert_eq!(b.links.len(), 1);
    assert_eq!(b.links[0].1, LinkTarget::Profile(UserId::from("U0ME")));

    // <#C0ENG|eng> and <#C0ENG>
    let mut b = ParagraphBuilder::new();
    assert_eq!(
        angle_token(&mut b, "<#C0ENG|eng>", &fixtures, &theme, DEFAULT_FLAGS),
        Some(12)
    );
    assert_eq!(b.text, "#eng");
    assert_eq!(b.links[0].1, LinkTarget::Channel(ChannelId::from("C0ENG")));

    let mut b = ParagraphBuilder::new();
    assert_eq!(
        angle_token(&mut b, "<#C0ENG>", &fixtures, &theme, DEFAULT_FLAGS),
        Some(8)
    );
    assert!(b.text.contains("engineering"));

    // <!here>, <!subteam^S1|@eng>, <!date^...|fallback>
    let mut b = ParagraphBuilder::new();
    assert_eq!(
        angle_token(&mut b, "<!here>", &fixtures, &theme, DEFAULT_FLAGS),
        Some(7)
    );
    assert_eq!(b.text, "@here");

    let mut b = ParagraphBuilder::new();
    assert_eq!(
        angle_token(
            &mut b,
            "<!subteam^S1|@eng>",
            &fixtures,
            &theme,
            DEFAULT_FLAGS
        ),
        Some(18)
    );
    assert_eq!(b.text, "@eng");

    let mut b = ParagraphBuilder::new();
    assert_eq!(
        angle_token(
            &mut b,
            "<!date^1700000000^{date_short}|Nov 14>",
            &fixtures,
            &theme,
            DEFAULT_FLAGS
        ),
        Some(38)
    );
    assert_eq!(b.text, "Nov 14");

    // <https://example.com|label> and <mailto:...>
    let mut b = ParagraphBuilder::new();
    assert_eq!(
        angle_token(
            &mut b,
            "<https://example.com/a&amp;b|*Docs*>",
            &fixtures,
            &theme,
            DEFAULT_FLAGS
        ),
        Some(36)
    );
    assert_eq!(b.text, "Docs");
    assert_eq!(b.runs[0].font.weight, FontWeight::BOLD);
    assert_eq!(
        b.links[0].1,
        LinkTarget::Url(SharedString::from("https://example.com/a&b"))
    );

    // A link label with a shortcode (a bot alert) or a multi-byte character before a colon.
    let mut b = ParagraphBuilder::new();
    angle_token(
        &mut b,
        "<https://example.com|:bar_chart: Panel>",
        &fixtures,
        &theme,
        DEFAULT_FLAGS,
    );
    assert_eq!(b.text, "📊 Panel");

    let mut b = ParagraphBuilder::new();
    angle_token(
        &mut b,
        "<https://example.com|Översikt: Q3>",
        &fixtures,
        &theme,
        DEFAULT_FLAGS,
    );
    assert_eq!(b.text, "Översikt: Q3");

    let mut b = ParagraphBuilder::new();
    assert_eq!(
        angle_token(
            &mut b,
            "<mailto:alice@example.com|Email>",
            &fixtures,
            &theme,
            DEFAULT_FLAGS
        ),
        Some(32)
    );
    assert_eq!(b.text, "Email");
    assert_eq!(
        b.links[0].1,
        LinkTarget::Url(SharedString::from("mailto:alice@example.com"))
    );

    // Unclosed or unknown angle brackets return None.
    let mut b = ParagraphBuilder::new();
    assert_eq!(
        angle_token(&mut b, "<unclosed", &fixtures, &theme, DEFAULT_FLAGS),
        None
    );
    assert_eq!(
        angle_token(&mut b, "<plain>", &fixtures, &theme, DEFAULT_FLAGS),
        None
    );
}

#[test]
fn push_mrkdwn_text_and_toggle_marker_enforce_verbatim_code_and_word_boundaries() {
    let normal_theme = test_run_theme(false);
    let verbatim_theme = test_run_theme(true);

    // Auto-links bare URLs, unescapes entities, and trims trailing punctuation.
    let mut b = ParagraphBuilder::new();
    push_mrkdwn_text(
        &mut b,
        "See https://example.com/a&amp;b, and http://test.dev).",
        &normal_theme,
        DEFAULT_FLAGS,
    );
    assert_eq!(b.text, "See https://example.com/a&b, and http://test.dev).");
    assert_eq!(b.links.len(), 2);
    assert_eq!(
        b.links[0].1,
        LinkTarget::Url(SharedString::from("https://example.com/a&b"))
    );
    assert_eq!(
        b.links[1].1,
        LinkTarget::Url(SharedString::from("http://test.dev"))
    );

    // linkify's boundary rules: balanced brackets stay in the URL, stray trailing punctuation
    // does not, and only `http(s)://` URLs link (no `www.` hosts, emails or `file.rs`).
    let cases: &[(&str, &[&str])] = &[
        (
            "(https://example.com/a_(b))",
            &["https://example.com/a_(b)"],
        ),
        ("https://example.com/q?x=1.", &["https://example.com/q?x=1"]),
        ("https://", &[]),
        ("www.example.com, alice@example.com and file.rs", &[]),
    ];
    for (text, urls) in cases {
        let mut b = ParagraphBuilder::new();
        push_mrkdwn_text(&mut b, text, &normal_theme, DEFAULT_FLAGS);
        assert_eq!(b.text, *text);
        let linked: Vec<&str> = b
            .links
            .iter()
            .map(|(range, _)| &b.text[range.clone()])
            .collect();
        assert_eq!(linked, *urls, "{text}");
    }

    // Verbatim mode and inline code skip auto-linking.
    let mut b_verb = ParagraphBuilder::new();
    push_mrkdwn_text(
        &mut b_verb,
        "visit https://example.com",
        &verbatim_theme,
        DEFAULT_FLAGS,
    );
    assert!(b_verb.links.is_empty());

    let mut b_code = ParagraphBuilder::new();
    push_mrkdwn_text(
        &mut b_code,
        "https://example.com",
        &normal_theme,
        RunFlags {
            code: true,
            ..DEFAULT_FLAGS
        },
    );
    assert!(b_code.links.is_empty());

    // toggle_marker word boundaries and code isolation
    let mut flags = DEFAULT_FLAGS;
    assert!(toggle_marker(&mut flags, '*', None, "bold*"));
    assert!(flags.bold);
    // Mid-word close is rejected, end-of-word close succeeds.
    assert!(!toggle_marker(&mut flags, '*', Some('d'), "more"));
    assert!(toggle_marker(&mut flags, '*', Some('d'), " rest"));
    assert!(!flags.bold);
    // Mid-word open is rejected.
    assert!(!toggle_marker(&mut flags, '_', Some('a'), "b_"));
    // Inside inline code, non-backtick markers do not toggle.
    flags.code = true;
    assert!(!toggle_marker(&mut flags, '*', None, "bold*"));
    assert!(toggle_marker(&mut flags, '`', Some('x'), " "));
    assert!(!flags.code);
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
    fixtures.users.push(bot_user);

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
    fixtures.users.push(ext_user);

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
    use std::time::Instant;

    let fixtures = crate::fixtures::load();
    let theme = test_run_theme(false);
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
    for i in 0..count {
        // 1. Syntax highlighting
        let lang = crate::syntax::Lang::detect(Some("rust"), None, code_sample);
        total_tokens += crate::syntax::highlight(lang, code_sample).len();

        // 2. Mrkdwn fence splitting + inline parsing
        for (is_code, segment) in mrkdwn_fences(mrkdwn_sample) {
            if is_code {
                total_tokens += crate::syntax::highlight(crate::syntax::Lang::Rust, segment).len();
            } else {
                let mut b = ParagraphBuilder::new();
                push_mrkdwn_text(&mut b, segment, &theme, DEFAULT_FLAGS);
                let _ = angle_token(&mut b, "<@U0ALICE>", &fixtures, &theme, DEFAULT_FLAGS);
                total_chars += b.text.len();
            }
        }

        // 3. Time & date formatting
        let msg_time =
            time::CivilTime::from_unix_with_offset(1_764_000_000 - (i as i64) * 7_200, 0);
        total_chars += time::card_stamp_at(&msg_time, &now, i % 2 == 0).len();
        total_chars += time::list_stamp_at(&msg_time, &now).len();
        total_chars += time::format_clock_civil(&msg_time, false).len();
    }
    let elapsed = start.elapsed();
    let ms = elapsed.as_secs_f64() * 1_000.0;
    let per_sec = (count as f64) / elapsed.as_secs_f64().max(1e-9);
    eprintln!(
        "bench 1000 messages (syntax + mrkdwn + time): {ms:.2} ms ({per_sec:.0} msgs/s; tokens={total_tokens}, chars={total_chars})"
    );
    assert!(total_tokens > 0 && total_chars > 0);
}
