//! Adversarial / fuzz-style invariant tests for `slackless-core` pure parsers and reducers.
//!
//! Ensures malformed, deeply nested, hostile Unicode, and randomized Slack payloads never panic
//! and preserve key structural invariants.

#![allow(clippy::unwrap_used)]

use serde_json::json;
use slackless_core::apply_event;
use slackless_core::domain::canvas_doc::CanvasDoc;
use slackless_core::domain::mrkdwn::{
    PlainDirectory, escape_mrkdwn, from_mrkdwn, mrkdwn_to_plain, mrkdwn_to_rich_text,
    parse_mrkdwn_spans, rich_text_to_mrkdwn, unescape_mrkdwn,
};
use slackless_core::domain::{slash, webvtt};
use slackless_core::store::Store;
use slackless_core::types::{Block, ChannelId, SlackEvent, TeamId, Ts};

struct Rng(u64);
impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn usize(&mut self, bound: usize) -> usize {
        (self.next_u64() as usize) % bound
    }
}

#[test]
fn mrkdwn_parsers_and_rich_text_roundtrip_survive_adversarial_and_randomized_inputs() {
    let tokens = [
        "*",
        "_",
        "~",
        "`",
        "```",
        ">",
        "\n",
        " ",
        "<@U123>",
        "<@U123|alice>",
        "<#C123|eng>",
        "<!here>",
        "<!channel>",
        "<!subteam^S1|@devs>",
        "<https://example.com|click>",
        "<mailto:a@b.c>",
        ":tada:",
        ":+1:",
        ":skin-tone-4:",
        "🔥",
        "👨‍👩‍👧‍👦",
        "åäö",
        "\u{200b}",
        "&amp;",
        "&lt;",
        "&gt;",
        "<",
        ">",
        "|",
        "\\",
        "https://a.b/c?x=1&y=2",
    ];

    for edge in [
        "",
        "   ",
        "``````",
        "***___~~~",
        "<<<<>>>>",
        "<@|>",
        "<https://>",
        ">>>\n>nested\n```code\nstill code",
        &"*".repeat(400),
        &"<@U1>".repeat(200),
    ] {
        let spans = parse_mrkdwn_spans(edge);
        let plain = mrkdwn_to_plain(edge);
        let doc = from_mrkdwn(edge, &PlainDirectory);
        let rt = mrkdwn_to_rich_text(edge, &PlainDirectory);
        let back = rich_text_to_mrkdwn(&[Block::RichText(rt)], &PlainDirectory);
        assert_eq!(unescape_mrkdwn(&escape_mrkdwn(edge)), edge);
        let _ = (spans.len(), plain.len(), doc, back);
    }

    let mut rng = Rng(0xA1B2_C3D4_E5F6_0718);
    for _ in 0..400 {
        let len = 1 + rng.usize(40);
        let mut input = String::new();
        for _ in 0..len {
            input.push_str(tokens[rng.usize(tokens.len())]);
        }
        let spans = parse_mrkdwn_spans(&input);
        let plain = mrkdwn_to_plain(&input);
        let rt = mrkdwn_to_rich_text(&input, &PlainDirectory);
        let back = rich_text_to_mrkdwn(&[Block::RichText(rt)], &PlainDirectory);
        assert_eq!(unescape_mrkdwn(&escape_mrkdwn(&input)), input);
        let _ = (spans.len(), plain.len(), back);
    }
}

#[test]
fn canvas_html_markdown_slash_and_webvtt_parsers_survive_hostile_inputs() {
    let hostile_html = [
        "",
        "<<<not html>>>",
        "<h1>Title</h1><p>Hello <b>world</b></p><ul><li>One</li><li>Two</li></ul>",
        "<table><tr><td>cell</td></tr></table><pre><code>fn main() {}</code></pre>",
        "<script>alert(1)</script><img src=x onerror=alert(1)><a href=\"javascript:alert(1)\">x</a>",
        &"<div>".repeat(120),
        "<p>Unclosed <b><i><s>tags",
    ];
    for raw in hostile_html {
        let doc = CanvasDoc::from_html("Test".into(), raw);
        let md = doc.to_markdown();
        // Round-tripping back from generated markdown must never panic.
        let _ = CanvasDoc::from_markdown("Test".into(), String::new(), &md);
    }

    for cmd in [
        "",
        "/",
        "/ ",
        "/shrug hi",
        "/status :coffee: working",
        "/topic",
        "/a/b/c",
    ] {
        let _ = slash::parse(cmd);
    }

    for vtt in [
        "",
        "WEBVTT\n\n00:00:01.000 --> 00:00:02.500\nHello world\n",
        "WEBVTT\n\nbad --> timestamp\nignored\n\n99:99:99.999 --> 00:00:00.000\nreversed\n",
        "not even vtt",
    ] {
        let _ = webvtt::parse_webvtt(vtt);
        let _ = webvtt::parse_webvtt_cues(vtt);
    }
}

#[test]
fn reducer_is_idempotent_on_duplicate_rtm_message_and_reaction_events() {
    let mut store = Store::open_in_memory().unwrap();
    let ws = TeamId::from("T0RED");
    let ch = ChannelId::from("C0ENG");
    let ts = Ts::from("1700000123.000100");

    let msg_event: SlackEvent = serde_json::from_value(json!({
        "type": "message",
        "channel": "C0ENG",
        "user": "U0ALICE",
        "text": "Idempotent delivery",
        "ts": "1700000123.000100"
    }))
    .unwrap();

    // Apply the exact same message event twice (e.g., reconnect replay).
    apply_event(&mut store, &ws, msg_event.clone()).unwrap();
    apply_event(&mut store, &ws, msg_event).unwrap();

    let stored = store.message(&ws, &ch, &ts).unwrap().unwrap();
    assert_eq!(stored.message.text, "Idempotent delivery");

    // Apply duplicate reaction_added events -> user count must NOT double-count.
    let react: SlackEvent = serde_json::from_value(json!({
        "type": "reaction_added",
        "user": "U0BOB",
        "reaction": "eyes",
        "item_user": "U0ALICE",
        "item": {
            "type": "message",
            "channel": "C0ENG",
            "ts": "1700000123.000100"
        },
        "event_ts": "1700000124.000100"
    }))
    .unwrap();
    apply_event(&mut store, &ws, react.clone()).unwrap();
    apply_event(&mut store, &ws, react).unwrap();

    let after = store.message(&ws, &ch, &ts).unwrap().unwrap();
    let eyes = after
        .message
        .reactions
        .iter()
        .find(|r| r.name == "eyes")
        .expect("eyes reaction recorded");
    assert_eq!(
        eyes.count, 1,
        "duplicate reaction_added for same user must be idempotent"
    );
}
