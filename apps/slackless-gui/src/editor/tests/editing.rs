//! Inline formatting, as-you-type conversion and undo.

use super::{span, type_text, typed};
use crate::editor::marks::{Attrs, Span, Style};

#[test]
fn toggling_a_format_on_a_selection_formats_exactly_that_range_and_back() {
    let mut editor = typed("hello world");
    editor.select(0..5);

    editor.toggle_style(Style::BOLD);
    assert_eq!(editor.doc().spans(), [span(0..5, Style::BOLD)]);
    assert_eq!(editor.active_style(), Style::BOLD);

    editor.toggle_style(Style::BOLD);
    assert!(editor.doc().spans().is_empty());
}

#[test]
fn a_format_armed_with_nothing_selected_applies_to_what_is_typed_next() {
    let mut editor = typed("a ");

    editor.toggle_style(Style::BOLD);
    type_text(&mut editor, "bc");
    editor.toggle_style(Style::BOLD);
    type_text(&mut editor, "d");

    assert_eq!(editor.doc().text(), "a bcd");
    assert_eq!(editor.doc().spans(), [span(2..4, Style::BOLD)]);
}

#[test]
fn moving_the_caret_disarms_a_pending_format() {
    let mut editor = typed("ab");
    editor.toggle_style(Style::ITALIC);

    editor.select(1..1);
    type_text(&mut editor, "x");

    assert!(editor.doc().spans().is_empty());
}

#[test]
fn text_typed_inside_a_format_keeps_it_but_a_link_is_not_extended_from_its_edge() {
    let mut editor = typed("see docs");
    editor.select(4..8);
    editor.toggle_style(Style::BOLD);
    editor.set_link("https://example.com");

    editor.select(6..6);
    type_text(&mut editor, "X");
    editor.select(9..9);
    type_text(&mut editor, "!");

    let link = Some("https://example.com".into());
    assert_eq!(
        editor.doc().spans(),
        [
            Span {
                range: 4..9,
                attrs: Attrs {
                    style: Style::BOLD,
                    link,
                },
            },
            span(9..10, Style::BOLD),
        ]
    );
}

#[test]
fn a_closing_marker_turns_the_pair_into_formatting_and_typing_continues_plain() {
    for (source, style) in [
        ("say *hi*", Style::BOLD),
        ("say _hi_", Style::ITALIC),
        ("say ~hi~", Style::STRIKE),
        ("say `hi`", Style::CODE),
    ] {
        let mut editor = typed(source);
        type_text(&mut editor, "!");

        assert_eq!(editor.doc().text(), "say hi!", "{source}");
        assert_eq!(editor.doc().spans(), [span(4..6, style)], "{source}");
    }
}

#[test]
fn markers_that_do_not_wrap_a_word_stay_as_typed() {
    for source in ["2*3*4", "snake_case_name", "a * b*", "x ~ y ~", "``"] {
        let editor = typed(source);

        assert_eq!(editor.doc().text(), source);
        assert!(editor.doc().spans().is_empty(), "{source}");
    }
}

#[test]
fn undo_after_a_conversion_brings_back_the_typed_markers() {
    let mut editor = typed("*hi*");

    assert!(editor.undo());

    assert_eq!(editor.doc().text(), "*hi*");
    assert!(editor.doc().spans().is_empty());
}

#[test]
fn undo_steps_back_a_word_at_a_time_and_redo_returns() {
    let mut editor = typed("hello world");

    assert!(editor.undo());
    assert_eq!(editor.doc().text(), "hello ");
    assert!(editor.undo());
    assert_eq!(editor.doc().text(), "");
    assert!(!editor.undo());

    assert!(editor.redo());
    assert_eq!(editor.doc().text(), "hello ");
}

#[test]
fn undo_reverts_a_format_toggle_without_touching_the_text() {
    let mut editor = typed("hello");
    editor.select(0..5);
    editor.toggle_style(Style::STRIKE);

    assert!(editor.undo());

    assert_eq!(editor.doc().text(), "hello");
    assert!(editor.doc().spans().is_empty());
}

#[test]
fn an_ime_commit_reported_as_a_whole_replacement_keeps_the_formatting() {
    let mut editor = typed("ab cd");
    editor.select(0..2);
    editor.toggle_style(Style::BOLD);

    editor.user_edit(0..5, "ab cd\u{3042}", 8..8);

    assert_eq!(editor.doc().text(), "ab cd\u{3042}");
    assert_eq!(editor.doc().spans(), [span(0..2, Style::BOLD)]);
}
