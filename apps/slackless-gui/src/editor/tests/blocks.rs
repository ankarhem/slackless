//! Lists, quotes and code blocks: opening, continuing, leaving and merging them.

use slackless_core::types::RichTextContainer;
use slackless_core::types::rich_text::{ListStyle, RichTextList};

use super::{kinds, plain, section, type_text, typed};
use crate::editor::marks::LineKind;

#[test]
fn a_line_prefix_opens_its_block_and_the_prefix_disappears() {
    for (prefix, kind) in [
        ("- ", LineKind::Bullet),
        ("* ", LineKind::Bullet),
        ("1. ", LineKind::Ordered),
        ("> ", LineKind::Quote),
        ("```", LineKind::Code),
    ] {
        let mut editor = typed(prefix);
        type_text(&mut editor, "x");

        let line = editor.doc().line(0);
        assert_eq!(line.kind, Some(kind), "{prefix}");
        assert_eq!(&editor.doc().text()[line.content], "x", "{prefix}");
    }
}

#[test]
fn three_backticks_after_text_start_a_code_block_on_the_next_line() {
    let editor = typed("run ```ls");

    assert_eq!(kinds(&editor), [None, Some(LineKind::Code)]);
    assert_eq!(editor.doc().to_mrkdwn(), "run \n```ls```");
}

#[test]
fn enter_starts_the_next_list_item_and_numbers_it() {
    let mut editor = typed("1. one");

    assert!(editor.enter());
    type_text(&mut editor, "two");

    assert_eq!(editor.doc().text(), "1. one\n2. two");
    assert_eq!(kinds(&editor), [Some(LineKind::Ordered); 2]);
}

#[test]
fn enter_on_an_empty_item_leaves_the_block() {
    for prefix in ["- ", "1. ", "> ", "```"] {
        let mut editor = typed(prefix);
        type_text(&mut editor, "x");
        editor.enter();

        assert!(editor.enter(), "{prefix}");

        assert_eq!(kinds(&editor)[1], None, "{prefix}");
        let caret = editor.doc().text().len();
        assert_eq!(editor.selection(), caret..caret, "{prefix}");
    }
}

#[test]
fn enter_on_a_paragraph_line_is_left_to_the_composer() {
    let mut editor = typed("hello");

    assert!(!editor.enter());
    assert!(!editor.soft_break());
    assert_eq!(editor.doc().text(), "hello");
}

#[test]
fn shift_enter_in_a_list_item_adds_a_line_to_the_same_item() {
    let mut editor = typed("- one");

    assert!(editor.soft_break());
    type_text(&mut editor, "more");

    assert_eq!(
        kinds(&editor),
        [Some(LineKind::Bullet), Some(LineKind::Soft)]
    );
    let block = editor.doc().to_rich_text();
    assert_eq!(
        block.elements,
        [RichTextContainer::List(RichTextList {
            style: ListStyle::Bullet,
            elements: vec![section(vec![plain("one\nmore")])],
            indent: Some(0),
            offset: None,
            border: Some(0),
        })]
    );
}

#[test]
fn joining_an_item_onto_the_line_above_drops_its_marker() {
    let mut editor = typed("x");
    type_text(&mut editor, "\n- y");
    assert_eq!(editor.doc().text(), "x\n\u{2022} y");

    // The engine erased the line break before the bullet.
    let diverged = editor.user_edit(1..2, "", 1..1);

    assert!(diverged);
    assert_eq!(editor.doc().text(), "xy");
    assert!(editor.doc().atoms().is_empty());
}

#[test]
fn text_typed_in_front_of_a_marker_lands_after_it() {
    let mut editor = typed("- b");

    // The caret sat before the bullet when the engine inserted "a" at offset 0.
    let diverged = editor.user_edit(0..0, "a", 1..1);

    assert!(diverged);
    assert_eq!(editor.doc().text(), "\u{2022} ab");
    assert_eq!(kinds(&editor), [Some(LineKind::Bullet)]);
    let caret = "\u{2022} a".len();
    assert_eq!(editor.selection(), caret..caret);
}

#[test]
fn the_caret_never_rests_in_front_of_a_marker() {
    let mut editor = typed("x");
    type_text(&mut editor, "\n- y");
    let bullet = "x\n".len();
    let text_start = "x\n\u{2022} ".len();

    // Home on the item's line asks for the offset in front of its bullet.
    editor.select(bullet..bullet);
    assert_eq!(editor.selection(), text_start..text_start);

    // Left from there goes to the end of the line above, and nowhere on the first line.
    assert!(editor.left_over_marker());
    assert_eq!(editor.selection(), 1..1);
    assert!(!editor.left_over_marker());
}

#[test]
fn lines_pasted_into_a_quote_are_all_quoted() {
    let mut editor = typed("> a");
    let caret = editor.selection().start;

    editor.user_edit(caret..caret, "b\nc\nd", caret + 5..caret + 5);

    assert_eq!(kinds(&editor), [Some(LineKind::Quote); 3]);
    assert_eq!(editor.doc().to_mrkdwn(), "&gt; ab\n&gt; c\n&gt; d");
}

#[test]
fn a_block_button_toggles_every_selected_line() {
    let mut editor = typed("a\nb\nc");
    editor.select(0..3);

    editor.toggle_block(LineKind::Ordered);
    assert_eq!(editor.doc().text(), "1. a\n2. b\nc");
    assert_eq!(editor.active_block(), Some(LineKind::Ordered));

    editor.toggle_block(LineKind::Ordered);
    assert_eq!(editor.doc().text(), "a\nb\nc");
}
