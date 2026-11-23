use super::render::*;
use gpui_kit::gpui::TextLayout;
use std::cell::RefCell;

use gpui_kit::base::TextSelection;
use gpui_kit::gpui::{
    Entity, Modifiers, MouseButton, Render, TestAppContext, VisualTestContext, Window, point, px,
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
                expanded.extend(list.data.read(cx).messages.clone());
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
