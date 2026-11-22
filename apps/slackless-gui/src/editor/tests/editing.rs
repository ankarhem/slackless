//! Inline markdown formatting, toggling, clearing, links, and undo.

use slackless_core::types::{RichTextContainer, RichTextElement, TextStyle};

use super::{type_text, typed};
use crate::editor::marks::Style;
use crate::editor::state::Editor;

#[test]
fn toggling_a_format_on_a_selection_wraps_and_unwraps_markdown_delimiters() {
    let mut editor = typed("hello world");
    editor.select(0..5);

    editor.toggle_style(Style::BOLD);
    assert_eq!(editor.doc().text(), "*hello* world");
    assert_eq!(editor.selection(), 1..6);
    assert_eq!(editor.active_style(), Style::BOLD);

    editor.toggle_style(Style::BOLD);
    assert_eq!(editor.doc().text(), "hello world");
    assert_eq!(editor.selection(), 0..5);
    assert_eq!(editor.active_style(), Style::NONE);
}

#[test]
fn clearing_formats_strips_inline_markers_and_serializes_underline() {
    let mut editor = typed("hello world");
    editor.select(0..5);

    editor.toggle_style(Style::BOLD);
    editor.toggle_style(Style::UNDERLINE);
    assert_eq!(editor.doc().text(), "*<u>hello</u>* world");
    assert_eq!(editor.active_style(), Style::BOLD.with(Style::UNDERLINE));

    let wire = |editor: &Editor| match editor.doc().to_rich_text().elements.first() {
        Some(RichTextContainer::Section(section)) => match section.elements.first() {
            Some(RichTextElement::Text(text)) => text.style,
            _ => None,
        },
        _ => None,
    };
    let underlined = wire(&editor);
    editor.clear_styles();
    assert_eq!(editor.doc().text(), "hello world");
    assert_eq!(
        underlined,
        Some(TextStyle {
            bold: true,
            underline: true,
            ..TextStyle::default()
        })
    );
    assert_eq!(wire(&editor), None);
}

#[test]
fn toggling_with_collapsed_caret_inserts_pair_and_toggling_again_steps_out() {
    let mut editor = typed("a ");

    editor.toggle_style(Style::BOLD);
    assert_eq!(editor.doc().text(), "a **");
    assert_eq!(editor.active_style(), Style::BOLD);
    type_text(&mut editor, "bc");
    assert_eq!(editor.doc().text(), "a *bc*");
    editor.toggle_style(Style::BOLD);
    type_text(&mut editor, "d");

    assert_eq!(editor.doc().text(), "a *bc*d");
    // Toggling off an empty armed pair deletes both markers.
    let mut empty = typed("ab");
    empty.toggle_style(Style::ITALIC);
    assert_eq!(empty.doc().text(), "ab__");
    empty.toggle_style(Style::ITALIC);
    assert_eq!(empty.doc().text(), "ab");
}

#[test]
fn links_wrap_and_unwrap_as_markdown_and_support_inline_styles() {
    let mut editor = typed("see docs");
    editor.select(4..8);
    editor.set_link("https://example.com");
    assert_eq!(editor.doc().text(), "see [docs](https://example.com)");
    assert!(editor.active_link().is_some());

    editor.select(5..9);
    editor.toggle_style(Style::BOLD);
    assert_eq!(editor.doc().text(), "see [*docs*](https://example.com)");

    editor.unlink();
    assert_eq!(editor.doc().text(), "see *docs*");
    assert!(editor.active_link().is_none());
}

#[test]
fn markdown_inline_syntax_parses_live_for_single_and_double_delimiters() {
    for (source, style) in [
        ("say *hi*!", Style::BOLD),
        ("say **hi**!", Style::BOLD),
        ("say _hi_!", Style::ITALIC),
        ("say ~hi~!", Style::STRIKE),
        ("say ~~hi~~!", Style::STRIKE),
        ("say `hi`!", Style::CODE),
        ("say <u>hi</u>!", Style::UNDERLINE),
    ] {
        let editor = typed(source);
        assert_eq!(editor.doc().text(), source, "buffer keeps raw markdown");
        let parsed = editor.doc().parse();
        assert_eq!(parsed.lines[0].inlines.len(), 1, "{source}");
        let inner_pos = parsed.lines[0].inlines[0].inner.start;
        assert_eq!(
            parsed.attrs_at(editor.doc(), inner_pos).style,
            style,
            "{source}"
        );
    }
}

#[test]
fn markers_that_do_not_wrap_a_word_stay_literal() {
    for source in ["2*3*4", "snake_case_name", "a * b*", "x ~ y ~", "``"] {
        let editor = typed(source);

        assert_eq!(editor.doc().text(), source);
        assert!(editor.doc().parse().lines[0].inlines.is_empty(), "{source}");
    }
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
fn undo_reverts_a_format_toggle_cleanly() {
    let mut editor = typed("hello");
    editor.select(0..5);
    editor.toggle_style(Style::STRIKE);
    assert_eq!(editor.doc().text(), "~hello~");

    assert!(editor.undo());
    assert_eq!(editor.doc().text(), "hello");
}

#[test]
fn an_ime_commit_reported_as_a_whole_replacement_keeps_markdown() {
    let mut editor = typed("ab cd");
    editor.select(0..2);
    editor.toggle_style(Style::BOLD);
    assert_eq!(editor.doc().text(), "*ab* cd");

    editor.user_edit(0..7, "*ab* cd\u{3042}", 10..10);

    assert_eq!(editor.doc().text(), "*ab* cd\u{3042}");
    let parsed = editor.doc().parse();
    assert_eq!(parsed.attrs_at(editor.doc(), 1).style, Style::BOLD);
}

#[test]
fn toggle_style_handles_multiline_whitespace_double_pairs_and_atoms() {
    // Whitespace-padded selection keeps trailing space outside `*`.
    let mut editor = typed("hello world");
    editor.select(0..6);
    editor.toggle_style(Style::BOLD);
    assert_eq!(editor.doc().text(), "*hello* world");

    // Multi-line selection wraps per line instead of creating unparseable cross-line `*`.
    let mut multi = typed("one\ntwo");
    multi.select(0..7);
    multi.toggle_style(Style::BOLD);
    assert_eq!(multi.doc().text(), "*one*\n*two*");

    // Double pair `**|**` deletes cleanly on toggle, while `*a*|*b*` does not corrupt.
    let mut dbl = typed("****");
    dbl.select(2..2);
    dbl.toggle_style(Style::BOLD);
    assert_eq!(dbl.doc().text(), "");

    let mut adj = typed("*a**b*");
    adj.select(3..3);
    assert_eq!(adj.active_style(), Style::NONE);

    // Inline code and links inside outer emphasis don't close the outer marker early.
    let nested_url = typed("_[docs](https://example.com/a_)_");
    let parsed = nested_url.doc().parse();
    assert_eq!(parsed.lines[0].inlines.len(), 2);
    let nested_code = typed("*check `x*` now*");
    let parsed_code = nested_code.doc().parse();
    assert_eq!(parsed_code.lines[0].inlines.len(), 2);

    // `set_link` preserves atoms inside selection and updates existing links without nesting.
    use crate::editor::marks::Atom;
    use slackless_core::types::UserId;
    let mut linked = typed("hi ");
    linked.insert_atom(3..3, Atom::User(UserId::from("U1")), "@Alice");
    linked.select(3..9);
    linked.set_link("https://one.dev");
    assert_eq!(linked.doc().text(), "hi [@Alice](https://one.dev) ");
    assert_eq!(linked.doc().atoms().len(), 1);
    linked.select(4..10);
    linked.set_link("https://two.dev");
    assert_eq!(linked.doc().text(), "hi [@Alice](https://two.dev) ");
}
