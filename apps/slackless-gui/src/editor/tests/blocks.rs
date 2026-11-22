//! Markdown blocks in Obsidian Live Preview: headings, lists, quotes, code fences, dividers.

use slackless_core::types::RichTextContainer;
use slackless_core::types::rich_text::{ListStyle, RichTextHeader, RichTextList};

use super::{Names, kinds, plain, section, type_text, typed};
use crate::editor::from_mrkdwn;
use crate::editor::marks::LineKind;

#[test]
fn markdown_line_prefixes_parse_directly_as_blocks() {
    for (prefix, kind) in [
        ("# ", LineKind::Heading(1)),
        ("## ", LineKind::Heading(2)),
        ("### ", LineKind::Heading(3)),
        ("- ", LineKind::Bullet),
        ("* ", LineKind::Bullet),
        ("1. ", LineKind::Ordered),
        ("> ", LineKind::Quote),
    ] {
        let mut editor = typed(prefix);
        type_text(&mut editor, "x");

        let parsed = editor.doc().parse();
        let line = &parsed.lines[0];
        assert_eq!(line.kind, Some(kind), "{prefix}");
        assert_eq!(&editor.doc().text()[line.content.clone()], "x", "{prefix}");
    }
}

#[test]
fn fenced_code_block_parses_fence_and_code_lines() {
    let editor = typed("```\nlet x = 1;\n```");

    assert_eq!(
        kinds(&editor),
        [
            Some(LineKind::Fence),
            Some(LineKind::Code),
            Some(LineKind::Fence)
        ]
    );
    assert_eq!(editor.doc().to_mrkdwn(), "```let x = 1;```");
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
    for prefix in ["- ", "1. ", "> ", "# "] {
        let mut editor = typed(prefix);
        type_text(&mut editor, "x");
        editor.enter();
        if prefix == "# " {
            // Heading Enter breaks directly to a plain paragraph.
            assert_eq!(kinds(&editor)[1], None, "{prefix}");
            continue;
        }
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
fn a_block_button_toggles_every_selected_line() {
    let mut editor = typed("a\nb\nc");
    editor.select(0..3);

    editor.toggle_block(LineKind::Ordered);
    assert_eq!(editor.doc().text(), "1. a\n2. b\nc");
    assert_eq!(editor.active_block(), Some(LineKind::Ordered));

    editor.toggle_block(LineKind::Ordered);
    assert_eq!(editor.doc().text(), "a\nb\nc");
}

#[test]
fn code_block_button_wraps_and_unwraps_fences() {
    let mut editor = typed("let x = 1;");
    editor.toggle_block(LineKind::Code);
    assert_eq!(editor.doc().text(), "```\nlet x = 1;\n```");
    assert_eq!(editor.active_block(), Some(LineKind::Code));

    editor.toggle_block(LineKind::Code);
    assert_eq!(editor.doc().text(), "let x = 1;");
    assert_eq!(editor.active_block(), None);
}

#[test]
fn a_heading_line_serializes_as_a_header_container_and_round_trips() {
    let mut editor = typed("title\nbody");
    editor.select(0..5);
    editor.toggle_block(LineKind::Heading(1));
    assert_eq!(editor.doc().text(), "# title\nbody");
    assert_eq!(editor.active_block(), Some(LineKind::Heading(1)));

    assert_eq!(
        editor.doc().to_rich_text().elements,
        vec![
            RichTextContainer::Header(RichTextHeader {
                level: Some(1),
                elements: vec![plain("title")],
            }),
            section(vec![plain("body")]),
        ]
    );

    let draft = editor.doc().to_mrkdwn();
    assert_eq!(draft, "# title\nbody");
    assert_eq!(from_mrkdwn(&draft, &Names).to_mrkdwn(), draft);

    // The dropdown's Paragraph clears the heading; lists and quotes stay.
    editor.select(0..7);
    editor.paragraph();
    assert_eq!(editor.doc().text(), "title\nbody");
    assert_eq!(editor.active_block(), None);
}

#[test]
fn insert_divider_places_rule_and_moves_caret_below() {
    let mut editor = typed("above");
    editor.insert_divider();
    type_text(&mut editor, "below");

    assert_eq!(editor.doc().text(), "above\n---\nbelow");
    assert_eq!(kinds(&editor), [None, Some(LineKind::Divider), None]);
}

#[test]
fn toggle_code_block_on_closing_fence_and_enter_inside_prefix_stay_well_formed() {
    // Toggling code block on the closing fence unwraps ONLY that block, not a second one below.
    let mut editor = typed("```\none\n```\nmid\n```\ntwo\n```");
    let close_pos = "```\none\n".len();
    editor.select(close_pos..close_pos);
    editor.toggle_block(LineKind::Code);
    assert_eq!(editor.doc().text(), "one\nmid\n```\ntwo\n```");

    // Enter at offset 0 of `- foo` pushes the item down instead of emitting `- - foo`.
    let mut list = typed("- foo");
    list.select(0..0);
    assert!(list.enter());
    assert_eq!(list.doc().text(), "\n- foo");

    // Deleting item 2 from an ordered list keeps item 3 recognized as ordered.
    let gap = typed("1. a\n3. c");
    assert_eq!(kinds(&gap), [Some(LineKind::Ordered); 2]);
}
