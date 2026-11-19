//! Frame codec: incoming text frames -> [`SlackEvent`], outgoing commands -> JSON text.

use serde_json::{Map, Value, json};
use slackless_types::{ChannelId, SlackEvent, Ts, UserId};

/// `type` strings of the frames this client sends.
pub mod frame_type {
    pub const PING: &str = "ping";
    pub const TICKLE: &str = "tickle";
    pub const PRESENCE_SUB: &str = "presence_sub";
    pub const PRESENCE_QUERY: &str = "presence_query";
    pub const USER_TYPING: &str = "user_typing";
}

/// Parses one websocket text frame. Any JSON object succeeds: unknown event types and drifted
/// payloads become [`SlackEvent::Unknown`]. Only non-JSON text is an error.
pub fn parse_frame(text: &str) -> Result<SlackEvent, serde_json::Error> {
    serde_json::from_str::<Value>(text).map(SlackEvent::from_value)
}

/// Why a `tickle` was sent; keeps the user "active".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TickleReason {
    Focus,
    Mousedown,
    Keydown,
    Polling,
    Other(String),
}

impl TickleReason {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Focus => "focus",
            Self::Mousedown => "mousedown",
            Self::Keydown => "keydown",
            Self::Polling => "polling",
            Self::Other(s) => s,
        }
    }
}

/// A frame to send. The client stamps every frame with a monotonic `id` at send time.
#[derive(Debug, Clone, PartialEq)]
pub enum Outgoing {
    /// App-level keepalive; the server answers `{"type":"pong","reply_to":id}`.
    Ping,
    /// Activity keepalive (send at most once per ~19 s).
    Tickle(TickleReason),
    /// Subscribe to presence for these users (replaces the previous subscription).
    PresenceSub(Vec<UserId>),
    /// One-shot presence lookup. Name from the task brief; payload shape not verified live.
    PresenceQuery(Vec<UserId>),
    /// Typing indicator in a channel or thread.
    UserTyping {
        channel: ChannelId,
        thread_ts: Option<Ts>,
    },
    /// Any other JSON object, sent as-is (plus `id`) for frames Slack adds later.
    Raw(Map<String, Value>),
}

impl Outgoing {
    /// Serializes the frame with `id` inserted.
    pub fn encode(&self, id: u64) -> String {
        let mut obj = match self {
            Self::Ping => json!({ "type": frame_type::PING }),
            Self::Tickle(reason) => {
                json!({ "type": frame_type::TICKLE, "reason": reason.as_str() })
            }
            Self::PresenceSub(ids) => json!({ "type": frame_type::PRESENCE_SUB, "ids": ids }),
            Self::PresenceQuery(ids) => json!({ "type": frame_type::PRESENCE_QUERY, "ids": ids }),
            Self::UserTyping { channel, thread_ts } => {
                let mut v = json!({ "type": frame_type::USER_TYPING, "channel": channel });
                if let Some(ts) = thread_ts {
                    v["thread_ts"] = json!(ts);
                }
                v
            }
            Self::Raw(map) => Value::Object(map.clone()),
        };
        obj["id"] = json!(id);
        obj.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slackless_types::event::MessageEvent;

    #[test]
    fn hello_maps_to_hello() {
        let ev = parse_frame(r#"{"type":"hello","fast_reconnect":true}"#).unwrap();
        assert!(matches!(ev, SlackEvent::Hello(h) if h.fast_reconnect));
    }

    #[test]
    fn message_event_maps_to_message() {
        let ev = parse_frame(
            r#"{"type":"message","channel":"C024BE91L","user":"U2147483697","text":"hello",
                "ts":"1700000000.000100","event_ts":"1700000000.000100","channel_type":"channel",
                "client_msg_id":"5f1c...","some_new_field":{"x":1}}"#,
        )
        .unwrap();
        let SlackEvent::Message(MessageEvent::Message(m)) = ev else {
            panic!("expected a plain message, got {ev:?}");
        };
        assert_eq!(m.text, "hello");
        assert_eq!(m.channel.as_ref().map(ChannelId::as_str), Some("C024BE91L"));
    }

    #[test]
    fn control_frames_map_to_their_variants() {
        assert!(matches!(
            parse_frame(r#"{"type":"goodbye"}"#).unwrap(),
            SlackEvent::Goodbye(_)
        ));
        assert!(matches!(
            parse_frame(r#"{"type":"reconnect_url","url":"wss://wss-primary.slack.com/websocket/abc"}"#).unwrap(),
            SlackEvent::ReconnectUrl(r) if r.url.ends_with("/abc")
        ));
        assert!(matches!(
            parse_frame(r#"{"type":"pong","reply_to":7}"#).unwrap(),
            SlackEvent::Pong(p) if p.reply_to == Some(7)
        ));
    }

    #[test]
    fn unknown_event_type_lands_in_catch_all() {
        let ev =
            parse_frame(r#"{"type":"update_thread_state","has_unreads":true,"mention_count":2}"#)
                .unwrap();
        let SlackEvent::Unknown { r#type, raw } = ev else {
            panic!("expected Unknown, got {ev:?}");
        };
        assert_eq!(r#type, "update_thread_state");
        assert_eq!(raw["mention_count"], 2);
    }

    #[test]
    fn non_json_is_an_error() {
        assert!(parse_frame("not json").is_err());
    }

    #[test]
    fn outgoing_frames_carry_id_and_type() {
        let parse = |s: String| serde_json::from_str::<Value>(&s).unwrap();
        assert_eq!(
            parse(Outgoing::Ping.encode(1)),
            json!({"type":"ping","id":1})
        );
        assert_eq!(
            parse(Outgoing::PresenceSub(vec![UserId::new("U1"), UserId::new("U2")]).encode(2)),
            json!({"type":"presence_sub","ids":["U1","U2"],"id":2})
        );
        assert_eq!(
            parse(
                Outgoing::UserTyping {
                    channel: ChannelId::new("C1"),
                    thread_ts: None
                }
                .encode(3)
            ),
            json!({"type":"user_typing","channel":"C1","id":3})
        );
        assert_eq!(
            parse(
                Outgoing::UserTyping {
                    channel: ChannelId::new("C1"),
                    thread_ts: Some(Ts::new("1.2"))
                }
                .encode(4)
            ),
            json!({"type":"user_typing","channel":"C1","thread_ts":"1.2","id":4})
        );
        assert_eq!(
            parse(Outgoing::Tickle(TickleReason::Polling).encode(5)),
            json!({"type":"tickle","reason":"polling","id":5})
        );
    }
}
