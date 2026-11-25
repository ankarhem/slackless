//! The two serializations: mrkdwn (and reading it back) and the rich_text block.

use slackless_core::types::rich_text::{
    BroadcastElement, BroadcastRange, ChannelElement, EmojiElement, LinkElement, ListStyle,
    RichTextList, RichTextPreformatted, RichTextQuote, TextElement, UserElement,
};
use slackless_core::types::{
    Block, ChannelId, RichTextContainer, RichTextElement, TextStyle, UserId,
};

use super::{Names, kinds, plain, section, type_text, typed};
use crate::editor::doc::Doc;
use crate::editor::marks::{Atom, LineKind};
use crate::editor::{from_mrkdwn, from_rich_text};

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
fn bare_urls_post_as_link_elements() {
    let editor = typed("see https://x.dev/a(b), or www.y.dev. done");
    let link = |url: &str| {
        RichTextElement::Link(LinkElement {
            url: url.to_owned(),
            text: None,
            style: None,
        })
    };

    assert_eq!(
        editor.doc().to_rich_text().elements,
        [section(vec![
            plain("see "),
            link("https://x.dev/a(b)"),
            plain(", or "),
            link("http://www.y.dev"),
            plain(". done"),
        ])]
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
    let start = editor.selection().start;
    type_text(&mut editor, "site");
    editor.select(start..editor.selection().end);
    editor.set_link("https://x.dev");
    type_text(&mut editor, " ");
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

#[test]
fn rich_text_round_trips_all_inline_styles_and_blocks() {
    let mut editor = typed("# Title\n## Sub\n### Small\n");
    type_text(
        &mut editor,
        "*bold* <u>under</u> _italic_ ~strike~ `code` [docs](https://example.com) ",
    );
    editor.insert_atom(editor.selection(), Atom::User(UserId::from("U1")), "@Alice");
    editor.insert_atom(
        editor.selection(),
        Atom::Channel(ChannelId::from("C1")),
        "#general",
    );
    editor.insert_atom(
        editor.selection(),
        Atom::Broadcast(BroadcastRange::Here),
        "@here",
    );
    editor.insert_atom(editor.selection(), Atom::Emoji("smile".into()), "\u{1F604}");

    type_text(&mut editor, "\n1. first");
    editor.soft_break();
    type_text(&mut editor, "cont");
    editor.enter();
    type_text(&mut editor, "second");
    editor.enter();
    editor.enter();
    type_text(&mut editor, "- bullet");
    editor.enter();
    editor.enter();
    type_text(&mut editor, "> quoted");
    editor.enter();
    editor.enter();
    type_text(&mut editor, "```\nlet x = 1;\nx\n```\n---\ntail");

    let original = editor.doc().trimmed();
    let rich = original.to_rich_text();
    let blocks: Vec<Block> = vec![rich.clone().into()];
    let restored = from_rich_text(&blocks, &Names).expect("restores rich_text doc");

    assert_eq!(restored, original);
    assert_eq!(restored.to_rich_text(), rich);
}

#[test]
fn from_rich_text_returns_none_without_rich_text_blocks() {
    assert!(from_rich_text(&[], &Names).is_none());
}

#[test]
fn code_styled_links_adjacent_runs_inline_code_urls_and_consecutive_fences_round_trip() {
    use slackless_core::types::RichTextBlock;

    let bold = TextStyle {
        bold: true,
        ..Default::default()
    };
    let bold_italic = TextStyle {
        bold: true,
        italic: true,
        ..Default::default()
    };
    let code = TextStyle {
        code: true,
        ..Default::default()
    };
    let rich = RichTextBlock {
        block_id: None,
        elements: vec![section(vec![
            RichTextElement::Link(LinkElement {
                url: "https://x.dev".into(),
                text: Some("api".into()),
                style: Some(code),
            }),
            plain(" "),
            RichTextElement::Text(TextElement {
                text: "foo".into(),
                style: Some(bold),
            }),
            RichTextElement::Text(TextElement {
                text: "bar".into(),
                style: Some(bold_italic),
            }),
            plain(" "),
            RichTextElement::Text(TextElement {
                text: "https://literal.dev".into(),
                style: Some(code),
            }),
        ])],
    };
    let restored = from_rich_text(&[rich.clone().into()], &Names).unwrap();
    assert_eq!(restored.to_rich_text(), rich);

    // Consecutive fenced code blocks stay distinct in `to_mrkdwn`.
    let two_blocks = typed("```\na\n```\n```\nb\n```");
    assert_eq!(two_blocks.doc().to_mrkdwn(), "```a```\n```b```");
}

#[test]
fn rich_text_handles_skin_tone_date_color_usergroup_team_offset_list_header_clamping_and_linkify() {
    use slackless_core::types::rich_text::{
        ColorElement, DateElement, RichTextHeader, TeamElement, UsergroupElement,
    };
    use slackless_core::types::{RichTextBlock, TeamId, UsergroupId};

    let rich = RichTextBlock {
        block_id: None,
        elements: vec![
            RichTextContainer::Header(RichTextHeader {
                level: Some(0), // clamps to 1
                elements: vec![plain("H0")],
            }),
            RichTextContainer::Header(RichTextHeader {
                level: Some(5), // clamps to 3
                elements: vec![plain("H5")],
            }),
            RichTextContainer::Header(RichTextHeader {
                level: None, // defaults to 2
                elements: vec![plain("HN")],
            }),
            section(vec![
                RichTextElement::Emoji(EmojiElement {
                    name: "thumbsup".into(),
                    unicode: None,
                    skin_tone: Some(3),
                    style: None,
                }),
                plain(" "),
                RichTextElement::Date(DateElement {
                    timestamp: 1_700_000_000,
                    format: "{date_short}".into(),
                    url: None,
                    fallback: Some("Nov 14".into()),
                }),
                plain(" "),
                RichTextElement::Color(ColorElement {
                    value: "#2BAC76".into(),
                }),
                plain(" "),
                RichTextElement::Usergroup(UsergroupElement {
                    usergroup_id: UsergroupId::from("S1"),
                    style: None,
                }),
                plain(" "),
                RichTextElement::Team(TeamElement {
                    team_id: TeamId::from("T99"),
                    style: None,
                }),
            ]),
            RichTextContainer::List(RichTextList {
                style: ListStyle::Ordered,
                elements: vec![section(vec![plain("fifth")]), section(vec![plain("sixth")])],
                indent: Some(0),
                offset: Some(4),
                border: Some(0),
            }),
        ],
    };

    let doc = from_rich_text(&[rich.into()], &Names).unwrap();
    assert!(doc.text().starts_with("# H0\n### H5\n## HN\n"));
    assert!(doc.text().contains("Nov 14 #2BAC76 @S1 T99"));
    assert!(doc.text().contains("5. fifth\n6. sixth"));

    // Emoji with skin_tone round-trips back to RichTextElement::Emoji with skin_tone = Some(3).
    let rt = doc.to_rich_text();
    let has_toned_emoji = rt.elements.iter().any(|c| match c {
        RichTextContainer::Section(s) => s.elements.iter().any(|el| {
            matches!(
                el,
                RichTextElement::Emoji(e) if e.name == "thumbsup" && e.skin_tone == Some(3)
            )
        }),
        _ => false,
    });
    assert!(has_toned_emoji, "emoji skin_tone must round-trip");

    // linkified: balanced parens kept, unbalanced outer parens stripped, trailing punct stripped, bare "https://" rejected.
    let link_doc = typed(
        "(https://en.wikipedia.org/wiki/Rust_(lang)) and https://example.com! and bare https:// only",
    );
    let rt = link_doc.doc().to_rich_text();
    let RichTextContainer::Section(sec) = &rt.elements[0] else {
        panic!("expected section");
    };
    let urls: Vec<&str> = sec
        .elements
        .iter()
        .filter_map(|el| match el {
            RichTextElement::Link(l) => Some(l.url.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        urls,
        vec![
            "https://en.wikipedia.org/wiki/Rust_(lang)",
            "https://example.com",
        ]
    );
}

#[test]
fn mrkdwn_read_and_write_handle_unclosed_fence_trailing_text_subteam_mailto_underline_and_retarget()
{
    // Unclosed fence auto-closes on read.
    let unclosed = from_mrkdwn("```\nlet x = 1;", &Names);
    assert_eq!(unclosed.text(), "```\n\nlet x = 1;\n```");

    // Trailing text after closing ``` on the same line becomes a paragraph line.
    let trailing = from_mrkdwn("```code```after *bold*", &Names);
    assert_eq!(trailing.to_mrkdwn(), "```code```\nafter *bold*");

    // <!subteam^...> with and without label + <mailto:...>
    let refs = from_mrkdwn(
        "ping <!subteam^S1|@eng> and <!subteam^S2|ops> and <!subteam^S3> <mailto:a@b.com> <mailto:c@d.com|Mail>",
        &Names,
    );
    assert!(refs.text().contains("@eng"));
    assert!(refs.text().contains("@ops"));
    assert!(refs.text().contains("@S3"));
    assert_eq!(
        refs.to_mrkdwn(),
        "ping <!subteam^S1> and <!subteam^S2> and <!subteam^S3> <mailto:a@b.com> <mailto:c@d.com|Mail>"
    );

    // Empty <u></u> stays literal; nested <u> stays well-formed without duplicating active underline.
    let empty_u = from_mrkdwn("<u></u>", &Names);
    assert_eq!(empty_u.text(), "<u></u>");
    let nested_u = from_mrkdwn("<u>outer <u>inner</u></u>", &Names);
    assert!(nested_u.to_draft().contains("<u>"));

    // Overlapping/nested styles close in LIFO order via retarget.
    let mut ed = typed("ab cd");
    ed.select(0..5);
    ed.toggle_style(crate::editor::marks::Style::BOLD);
    ed.select(1..3); // "ab" inside "*ab cd*"
    ed.toggle_style(crate::editor::marks::Style::ITALIC);
    assert_eq!(ed.doc().to_mrkdwn(), "*_ab_ cd*");
}
