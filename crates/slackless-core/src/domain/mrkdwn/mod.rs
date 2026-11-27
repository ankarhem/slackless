//! Pure, GPUI-free Slack `mrkdwn` <-> `RichTextBlock` document model, serializer, reader,
//! and inline span parser shared by GUI and CLI.

mod doc;
mod parse;
mod rich_text;
mod spans;

pub use doc::{Atom, Attrs, Doc, LineKind, PLAIN, Placed, Style};
pub use parse::{
    InlineConstruct, InlineKind, ParsedDoc, ParsedLine, Piece, StyledRun, delimiter_ranges,
    empty_pair_at,
};
pub use rich_text::from_rich_text;
#[cfg(any(test, feature = "test-support"))]
pub use rich_text::{mrkdwn_to_rich_text, rich_text_to_mrkdwn};
#[cfg(any(test, feature = "test-support"))]
pub use spans::escape_mrkdwn;
pub use spans::{
    Directory, MrkdwnSpan, PlainDirectory, from_mrkdwn, mrkdwn_to_plain, mrkdwn_to_plain_with,
    parse_mrkdwn_spans, parse_shortcode, replace_shortcodes_with, unescape_mrkdwn,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::rich_text::BroadcastRange;
    use crate::types::{Block, ChannelId, UserId, UsergroupId};

    struct TestDir;

    impl Directory for TestDir {
        fn mention(&self, id: &UserId) -> String {
            match id.as_str() {
                "U1" => "alice".into(),
                other => other.into(),
            }
        }
        fn channel_name(&self, id: &ChannelId) -> String {
            match id.as_str() {
                "C1" => "general".into(),
                other => other.into(),
            }
        }
        fn usergroup_handle(&self, id: &UsergroupId) -> String {
            match id.as_str() {
                "S1" => "eng".into(),
                other => other.into(),
            }
        }
        fn emoji_text(&self, shortcode: &str) -> Option<String> {
            match shortcode {
                "rocket" => Some("🚀".into()),
                "wave::skin-tone-3" => Some("👋🏼".into()),
                _ => None,
            }
        }
    }

    #[test]
    fn escape_and_unescape_roundtrip() {
        let raw = "a < b & c > d";
        let escaped = escape_mrkdwn(raw);
        assert_eq!(escaped, "a &lt; b &amp; c &gt; d");
        assert_eq!(unescape_mrkdwn(&escaped), raw);
        assert!(matches!(
            unescape_mrkdwn("plain text"),
            std::borrow::Cow::Borrowed("plain text")
        ));
    }

    #[test]
    fn mrkdwn_and_rich_text_roundtrip_all_constructs() {
        let input = "*bold* _italic_ ~strike~ `code` <@U1> <#C1> <!subteam^S1|@eng> <!here> :rocket: <https://example.com|Example>";
        let doc = from_mrkdwn(input, &TestDir);
        let rt = doc.to_rich_text();
        let rebuilt = from_rich_text(&[Block::RichText(rt.clone())], &TestDir).expect("rich_text");
        assert_eq!(rebuilt.to_mrkdwn(), doc.to_mrkdwn());
        assert_eq!(
            rich_text_to_mrkdwn(&[Block::RichText(rt)], &TestDir).as_deref(),
            Some(doc.to_mrkdwn().as_str())
        );

        let rt2 = mrkdwn_to_rich_text("> quote\n- bullet\n1. ordered\n---\n```code```", &TestDir);
        assert!(rt2.elements.len() >= 4);
    }

    #[test]
    fn parses_mrkdwn_spans_and_plain_text() {
        let raw = "Hello *world* &amp; <@U1|alice> in <#C1|general> <!here> <https://slack.com|Slack> :wave::skin-tone-3: <!date^1700000000^{date}|Nov 14>";
        let spans = parse_mrkdwn_spans(raw);
        assert!(spans.iter().any(|s| matches!(
            s,
            MrkdwnSpan::Text { text, style } if text == "world" && style.contains(Style::BOLD)
        )));
        assert!(spans.iter().any(|s| matches!(
            s,
            MrkdwnSpan::UserMention { id, label, .. } if id.as_str() == "U1" && label.as_deref() == Some("alice")
        )));
        assert!(spans.iter().any(|s| matches!(
            s,
            MrkdwnSpan::ChannelMention { id, name, .. } if id.as_str() == "C1" && name.as_deref() == Some("general")
        )));
        assert!(spans.iter().any(|s| matches!(
            s,
            MrkdwnSpan::Broadcast {
                range: BroadcastRange::Here,
                ..
            }
        )));
        assert!(spans.iter().any(|s| matches!(
            s,
            MrkdwnSpan::Link { url, label, .. } if url == "https://slack.com" && label.as_deref() == Some("Slack")
        )));
        assert!(spans.iter().any(|s| matches!(
            s,
            MrkdwnSpan::Emoji { shortcode, .. } if shortcode == "wave::skin-tone-3"
        )));
        assert!(spans.iter().any(|s| matches!(
            s,
            MrkdwnSpan::Date { timestamp: 1_700_000_000, fallback, .. } if fallback == "Nov 14"
        )));

        assert_eq!(
            mrkdwn_to_plain(raw),
            "Hello world & @alice in #general @here Slack :wave::skin-tone-3: Nov 14"
        );

        // A link label wrapped whole in markers takes their style, as Slack draws
        // `<url|*TypeError*>` as a bold link; partly styled labels stay literal.
        let link = |label: &str, style: Style| MrkdwnSpan::Link {
            url: "https://x.dev".into(),
            label: Some(label.into()),
            style,
        };
        for (raw, span) in [
            (
                "<https://x.dev|*TypeError*>",
                link("TypeError", Style::BOLD),
            ),
            (
                "<https://x.dev|_*both*_>",
                link("both", Style::ITALIC.with(Style::BOLD)),
            ),
            (
                "_<https://x.dev|*:rocket: go*>_",
                link(":rocket: go", Style::ITALIC.with(Style::BOLD)),
            ),
            (
                "<https://x.dev|a *b* &amp; c>",
                link("a *b* & c", Style::NONE),
            ),
            (
                "<https://x.dev|*a* and *b*>",
                link("*a* and *b*", Style::NONE),
            ),
        ] {
            assert_eq!(parse_mrkdwn_spans(raw), vec![span], "{raw}");
        }
        assert_eq!(mrkdwn_to_plain("<https://x.dev|*TypeError*>"), "TypeError");
    }

    /// A directory names the references it knows; one it does not know (it echoes the id)
    /// falls back to the label the text carries, else the id, as without a directory.
    #[test]
    fn plain_text_resolves_references_through_the_directory() {
        let raw = "<@U1> <@U9|zoe> <@U8> <#C1> <#C9|design> <!subteam^S1> <!subteam^S9|@ops> \
                   <!channel> <https://slack.com|Slack> <https://example.com> :rocket: *bold*";
        assert_eq!(
            mrkdwn_to_plain_with(raw, &TestDir),
            "@alice @zoe @U8 #general #design @eng @ops @channel Slack https://example.com :rocket: bold"
        );
        // Without a directory every reference falls back to its label, else its id.
        assert_eq!(
            mrkdwn_to_plain(raw),
            "@U1 @zoe @U8 #C1 #design @S1 @ops @channel Slack https://example.com :rocket: bold"
        );
        // An empty label is no label.
        assert_eq!(mrkdwn_to_plain("<@U1|> <#C1|>"), "@U1 #C1");
    }

    #[test]
    fn draft_preserves_underline_while_wire_mrkdwn_strips_tag() {
        let doc = Doc::from("hello <u>world</u>");
        assert_eq!(doc.to_draft(), "hello <u>world</u>");
        assert_eq!(doc.to_mrkdwn(), "hello world");
    }

    #[test]
    fn wrapped_styles_skip_markers_in_code_and_links_and_roundtrip_edge_cases() {
        // Markers inside inline code and `<...>` links must not prematurely close outer styles.
        let spans = parse_mrkdwn_spans("*bold with `a*b` inside*");
        assert_eq!(
            spans,
            vec![
                MrkdwnSpan::Text {
                    text: "bold with ".into(),
                    style: Style::BOLD,
                },
                MrkdwnSpan::Text {
                    text: "a*b".into(),
                    style: Style::BOLD.with(Style::CODE),
                },
                MrkdwnSpan::Text {
                    text: " inside".into(),
                    style: Style::BOLD,
                },
            ]
        );
        let italic_link = parse_mrkdwn_spans("_see <https://ex.com/a_b> now_");
        assert_eq!(
            italic_link,
            vec![
                MrkdwnSpan::Text {
                    text: "see ".into(),
                    style: Style::ITALIC,
                },
                MrkdwnSpan::Link {
                    url: "https://ex.com/a_b".into(),
                    label: None,
                    style: Style::ITALIC,
                },
                MrkdwnSpan::Text {
                    text: " now".into(),
                    style: Style::ITALIC,
                },
            ]
        );

        // `from_mrkdwn` preserves `*bold `a*b` end*`, empty link labels `<url|>`, and `[` in link labels.
        let doc_code = from_mrkdwn("*bold `a*b` end*", &TestDir);
        assert_eq!(doc_code.to_mrkdwn(), "*bold `a*b` end*");

        let doc_empty_label = from_mrkdwn("<https://example.com|>", &TestDir);
        assert_eq!(doc_empty_label.to_mrkdwn(), "<https://example.com>");

        let doc_brackets = from_mrkdwn("<https://example.com|[BUG] fix>", &TestDir);
        assert_eq!(doc_brackets.to_mrkdwn(), "<https://example.com|[BUG] fix>");

        // `linkified` handles short `www.` URLs and strips unmatched trailing `>` / `]`.
        let rt = mrkdwn_to_rich_text("visit <www.x.io> and [http://a.co]", &TestDir);
        let json = serde_json::to_string(&rt).unwrap();
        assert!(json.contains("\"url\":\"http://www.x.io\""), "{json}");
        assert!(json.contains("\"url\":\"http://a.co\""), "{json}");
        assert!(!json.contains("www.x.io>"), "{json}");
        assert!(!json.contains("a.co]"), "{json}");
    }

    #[test]
    fn mrkdwn_link_entities_empty_labels_subteams_and_shortcodes() {
        // 1. Plain and bold link labels must unescape entities exactly once (no double-unescape).
        assert_eq!(
            parse_mrkdwn_spans("<https://x.dev|&amp;lt;script&amp;gt;>"),
            vec![MrkdwnSpan::Link {
                url: "https://x.dev".into(),
                label: Some("&lt;script&gt;".into()),
                style: Style::NONE,
            }]
        );
        assert_eq!(
            parse_mrkdwn_spans("<https://x.dev|*a &amp;lt; b*>"),
            vec![MrkdwnSpan::Link {
                url: "https://x.dev".into(),
                label: Some("a &lt; b".into()),
                style: Style::BOLD,
            }]
        );
        assert_eq!(
            parse_mrkdwn_spans("<https://x.dev|*see &lt;https://b.dev&gt;*>"),
            vec![MrkdwnSpan::Link {
                url: "https://x.dev".into(),
                label: Some("see <https://b.dev>".into()),
                style: Style::BOLD,
            }]
        );

        // 2. Empty labels after `|` normalize to `None` so URLs and channel fallbacks aren't lost.
        assert_eq!(
            parse_mrkdwn_spans("<https://x.dev|>"),
            vec![MrkdwnSpan::Link {
                url: "https://x.dev".into(),
                label: None,
                style: Style::NONE,
            }]
        );
        assert_eq!(mrkdwn_to_plain("<https://x.dev|>"), "https://x.dev");
        assert_eq!(
            parse_mrkdwn_spans("<#C1|>"),
            vec![MrkdwnSpan::ChannelMention {
                id: ChannelId::from("C1"),
                name: None,
                style: Style::NONE,
            }]
        );

        // 3. `from_mrkdwn` subteam references fall back on empty label and unescape entities.
        let sub_empty = from_mrkdwn("<!subteam^S1|>", &TestDir);
        assert_eq!(sub_empty.text, "@eng");
        let sub_entity = from_mrkdwn("<!subteam^S1|@r&amp;d>", &TestDir);
        assert_eq!(sub_entity.text, "@r&d");

        // 4. `replace_shortcodes_with` boundary cases (adjacent colons, 40-char vs 41-char,
        //    multibyte after skin-tone).
        let lookup = |name: &str| match name {
            "rocket" => Some("🚀".to_owned()),
            "wave::skin-tone-3" => Some("👋🏽".to_owned()),
            s if s.len() == 40 => Some("OK40".to_owned()),
            _ => None,
        };
        assert_eq!(
            replace_shortcodes_with(":unknown:rocket:", &lookup),
            ":unknown🚀"
        );
        let name40 = "a".repeat(40);
        let name41 = "a".repeat(41);
        assert_eq!(
            replace_shortcodes_with(&format!(":{name40}: :{name41}:"), &lookup),
            format!("OK40 :{name41}:")
        );
        assert_eq!(
            replace_shortcodes_with(":wave::skin-tone-3: :wave::skin-tone-é:", &lookup),
            "👋🏽 :wave::skin-tone-é:"
        );
    }
}
