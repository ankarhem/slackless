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
pub use rich_text::{from_rich_text, mrkdwn_to_rich_text, rich_text_to_mrkdwn};
pub use spans::{
    Directory, MrkdwnSpan, PlainDirectory, escape_mrkdwn, from_mrkdwn, mrkdwn_to_plain,
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
    }

    #[test]
    fn draft_preserves_underline_while_wire_mrkdwn_strips_tag() {
        let doc = Doc::from("hello <u>world</u>");
        assert_eq!(doc.to_draft(), "hello <u>world</u>");
        assert_eq!(doc.to_mrkdwn(), "hello world");
    }
}
