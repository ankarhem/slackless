//! Completion: where a lookup opens, how candidates rank, and what a pick leaves behind.

use slackless_core::types::UserId;

use super::{type_text, typed};
use crate::editor::marks::Atom;
use crate::editor::{Lookup, rank, trigger};

#[test]
fn a_picked_mention_replaces_its_query_and_is_followed_by_a_space() {
    let mut editor = typed("hi @al");
    let query = trigger(editor.doc(), 6).map(|trigger| trigger.range);
    assert_eq!(query, Some(3..6));

    editor.insert_atom(3..6, Atom::User(UserId::from("U1")), "@Alice");

    assert_eq!(editor.doc().text(), "hi @Alice ");
    assert_eq!(editor.selection(), 10..10);
    assert_eq!(editor.doc().to_mrkdwn(), "hi <@U1> ");
}

#[test]
fn erasing_a_mention_removes_the_whole_chip() {
    let mut editor = typed("hi ");
    editor.insert_atom(3..3, Atom::User(UserId::from("U1")), "@Alice");

    // The engine erases a token as one unit.
    editor.user_edit(3..9, "", 3..3);

    assert!(editor.doc().atoms().is_empty());
    assert_eq!(editor.doc().to_mrkdwn(), "hi  ");
}

#[test]
fn a_lookup_opens_only_where_its_trigger_starts_a_word() {
    for (source, expected) in [
        ("hi @al", Some((Lookup::People, "al"))),
        ("@", Some((Lookup::People, ""))),
        ("in #gen", Some((Lookup::Channels, "gen"))),
        ("ok :sm", Some((Lookup::Emoji, "sm"))),
        ("(:tada", Some((Lookup::Emoji, "tada"))),
        ("ok :s", None),
        ("at 12:30", None),
        ("mail a@b", None),
        ("@al ice", None),
    ] {
        let editor = typed(source);
        let caret = editor.selection().start;

        let found = trigger(editor.doc(), caret).map(|found| (found.lookup, found.query));

        assert_eq!(found, expected, "{source}");
    }
}

#[test]
fn no_lookup_opens_inside_code() {
    let mut inline = typed("`a b`");
    inline.select(2..2);
    type_text(&mut inline, "@c");
    let block = typed("```@al");

    assert_eq!(trigger(inline.doc(), 4), None);
    assert_eq!(trigger(block.doc(), block.selection().start), None);
}

#[test]
fn candidates_rank_by_where_the_query_matches() {
    assert_eq!(rank("Alice Smith", "al"), Some(0));
    assert_eq!(rank("Bob Alston", "al"), Some(1));
    assert_eq!(rank("Malory", "al"), Some(2));
    assert_eq!(rank("Bob", "al"), None);
    assert_eq!(rank("ÅSA", "ås"), Some(0));
}
