//! The two serializations: mrkdwn (and reading it back) and the rich_text block.

use slackless_core::types::rich_text::{
    BroadcastElement, BroadcastRange, ChannelElement, EmojiElement, LinkElement, ListStyle,
    RichTextList, RichTextPreformatted, RichTextQuote, TextElement, UserElement,
};
use slackless_core::types::{ChannelId, RichTextContainer, RichTextElement, TextStyle, UserId};

use super::{Names, kinds, plain, section, type_text, typed};
use crate::editor::doc::Doc;
use crate::editor::from_mrkdwn;
use crate::editor::marks::{Atom, LineKind};

#[test]
fn literal_text_is_escaped_in_mrkdwn_and_untouched_in_rich_text() {
    let editor = typed("a < b & c > d");

    assert_eq!(editor.doc().to_mrkdwn(), "a &lt; b &amp; c &gt; d");
    assert_eq!(
        editor.doc().to_rich_text().elements,
        [section(vec![plain("a < b & c > d")])]
    );
}

#[test]
fn rich_text_carries_formats_links_references_and_blocks() {
    let source = "*b* _i_ ~s~ `c` <https://x.dev|site> <@U1> <#C1> <!here> :smile:\n\
                  second\n\
                  \u{2022} one\n\
                  \u{2022} two\n\
                  &gt; quoted\n\
                  ```let x = 1;\n\
                  x```";

    let block = from_mrkdwn(source, &Names).to_rich_text();

    let styled = |text: &str, style: TextStyle| {
        RichTextElement::Text(TextElement {
            text: text.to_owned(),
            style: Some(style),
        })
    };
    let on = TextStyle::default;
    assert_eq!(
        block.elements,
        [
            section(vec![
                styled("b", TextStyle { bold: true, ..on() }),
                plain(" "),
                styled(
                    "i",
                    TextStyle {
                        italic: true,
                        ..on()
                    }
                ),
                plain(" "),
                styled(
                    "s",
                    TextStyle {
                        strike: true,
                        ..on()
                    }
                ),
                plain(" "),
                styled("c", TextStyle { code: true, ..on() }),
                plain(" "),
                RichTextElement::Link(LinkElement {
                    url: "https://x.dev".to_owned(),
                    text: Some("site".to_owned()),
                    style: None,
                }),
                plain(" "),
                RichTextElement::User(UserElement {
                    user_id: UserId::from("U1"),
                    style: None,
                }),
                plain(" "),
                RichTextElement::Channel(ChannelElement {
                    channel_id: ChannelId::from("C1"),
                    style: None,
                }),
                plain(" "),
                RichTextElement::Broadcast(BroadcastElement {
                    range: BroadcastRange::Here,
                    style: None,
                }),
                plain(" "),
                RichTextElement::Emoji(EmojiElement {
                    name: "smile".to_owned(),
                    unicode: None,
                    skin_tone: None,
                    style: None,
                }),
                plain("\nsecond"),
            ]),
            RichTextContainer::List(RichTextList {
                style: ListStyle::Bullet,
                elements: vec![section(vec![plain("one")]), section(vec![plain("two")])],
                indent: Some(0),
                offset: None,
                border: Some(0),
            }),
            RichTextContainer::Quote(RichTextQuote {
                elements: vec![plain("quoted")],
                border: None,
            }),
            RichTextContainer::Preformatted(RichTextPreformatted {
                elements: vec![plain("let x = 1;\nx")],
                border: Some(0),
                language: None,
            }),
        ]
    );
}

#[test]
fn a_draft_reads_back_from_its_mrkdwn_unchanged() {
    for source in [
        "plain text",
        "*bold* and _italic_ and ~struck~ and `code`",
        "*_both_*",
        "a &lt; b &amp;&amp; c",
        "<https://x.dev|site> and <https://y.dev>",
        "hey <@U1> in <#C1> <!channel> :smile: :partyparrot:",
        "1. one\n    still one\n2. two\n\nafter",
        "\u{2022} item\n&gt; quote\n&gt; \n```code &lt;T&gt;\n\n  indented```\ntail",
    ] {
        let doc = from_mrkdwn(source, &Names);

        assert_eq!(doc.to_mrkdwn(), source);
    }
}

#[test]
fn an_edited_draft_survives_the_mrkdwn_round_trip_exactly() {
    let mut editor = typed("- *first* item");
    editor.enter();
    type_text(&mut editor, "see ");
    editor.set_link("https://x.dev");
    editor.insert_atom(editor.selection(), Atom::Emoji("smile".into()), "\u{1F604}");

    let read = from_mrkdwn(&editor.doc().to_mrkdwn(), &Names);

    assert_eq!(&read, editor.doc());
}

#[test]
fn text_that_only_looks_like_markup_reads_back_literally() {
    for source in [
        "2*3*4",
        "snake_case_name",
        "3. not a list",
        ":nosuchemoji:",
        "a <b> c",
    ] {
        let doc = from_mrkdwn(source, &Names);

        assert_eq!(doc, Doc::from(source), "{source}");
    }
}

#[test]
fn what_is_sent_has_no_surrounding_blank_space_or_trailing_empty_item() {
    let mut editor = typed(" \n- one");
    editor.enter();
    assert_eq!(kinds(&editor).last(), Some(&Some(LineKind::Bullet)));

    let sent = editor.doc().trimmed();

    assert_eq!(sent.to_mrkdwn(), "\u{2022} one");
    assert!(!sent.is_blank());
    assert!(typed("- ").doc().is_blank());
    assert!(typed(" \n ").doc().is_blank());
}
