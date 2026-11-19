//! Websocket (RTM / flannel) events, tagged by their `type` field.
//!
//! Parsing never fails on a well-formed JSON object: an event type this crate does not know, or a
//! known type whose payload drifted beyond what the structs tolerate, becomes
//! [`SlackEvent::Unknown`] with the raw JSON kept for logging or later handling.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::conversation::Conversation;
use crate::de::null_default;
use crate::ids::{ChannelId, FileId, TeamId, Ts, UserId};
use crate::macros::{serialize_tagged, tagged_enum};
use crate::message::{Message, MessageSubtype};
use crate::user::{DndStatus, Presence, User};

tagged_enum! {
    /// One frame received on the realtime websocket.
    pub enum SlackEvent (tag = "type", default = None) {
        // Connection control.
        "hello" => Hello(Hello),
        "goodbye" => Goodbye(Goodbye),
        "reconnect_url" => ReconnectUrl(ReconnectUrl),
        "pong" => Pong(Pong),
        "error" => Error(SocketError),
        // Messages (every subtype; see [`MessageEvent`]).
        "message" => Message(MessageEvent),
        // Reactions, pins, stars.
        "reaction_added" => ReactionAdded(ReactionEvent),
        "reaction_removed" => ReactionRemoved(ReactionEvent),
        "pin_added" => PinAdded(PinEvent),
        "pin_removed" => PinRemoved(PinEvent),
        "star_added" => StarAdded(StarEvent),
        "star_removed" => StarRemoved(StarEvent),
        // Read state.
        "channel_marked" => ChannelMarked(MarkedEvent),
        "im_marked" => ImMarked(MarkedEvent),
        "group_marked" => GroupMarked(MarkedEvent),
        "mpim_marked" => MpimMarked(MarkedEvent),
        "thread_marked" => ThreadMarked(ThreadMarkedEvent),
        // Typing and presence.
        "user_typing" => UserTyping(UserTyping),
        "presence_change" => PresenceChange(PresenceChange),
        "manual_presence_change" => ManualPresenceChange(ManualPresenceChange),
        "dnd_updated" => DndUpdated(DndUpdated),
        "dnd_updated_user" => DndUpdatedUser(DndUpdated),
        // Conversation lifecycle.
        "channel_created" => ChannelCreated(ChannelObjectEvent),
        "channel_joined" => ChannelJoined(ChannelObjectEvent),
        "channel_rename" => ChannelRename(ChannelObjectEvent),
        "group_rename" => GroupRename(ChannelObjectEvent),
        "channel_archive" => ChannelArchive(ChannelRefEvent),
        "channel_unarchive" => ChannelUnarchive(ChannelRefEvent),
        "channel_left" => ChannelLeft(ChannelRefEvent),
        "channel_deleted" => ChannelDeleted(ChannelRefEvent),
        "member_joined_channel" => MemberJoinedChannel(MemberChannelEvent),
        "member_left_channel" => MemberLeftChannel(MemberChannelEvent),
        // Users, emoji, prefs.
        "user_change" => UserChange(UserChange),
        "user_profile_changed" => UserProfileChanged(UserChange),
        "emoji_changed" => EmojiChanged(EmojiChanged),
        "pref_change" => PrefChange(PrefChange),
    }
}

/// Socket is live. `fast_reconnect` is true when resumed via a `reconnect_url`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    #[serde(default, deserialize_with = "null_default")]
    pub fast_reconnect: bool,
}

/// Server is closing the socket; reconnect.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Goodbye {}

/// URL to dial on the next reconnect.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconnectUrl {
    pub url: String,
}

/// Reply to an outgoing `ping`; `reply_to` echoes its `id`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pong {
    #[serde(default)]
    pub reply_to: Option<u64>,
}

/// Socket-level error (`code` 1 = URL expired, 17 = rate limited, 401 = invalid auth).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SocketError {
    #[serde(default, deserialize_with = "null_default")]
    pub error: SocketErrorDetail,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SocketErrorDetail {
    #[serde(default)]
    pub code: Option<i64>,
    #[serde(default)]
    pub msg: Option<String>,
}

/// A `type: "message"` event, split by the subtypes that change existing state.
///
/// New messages of every other subtype (including `thread_broadcast`, `bot_message`, and
/// subtypes Slack adds later) arrive as [`MessageEvent::Message`]; check `Message::subtype`.
#[derive(Debug, Clone, PartialEq)]
pub enum MessageEvent {
    Message(Box<Message>),
    Changed(MessageChanged),
    Deleted(MessageDeleted),
    Replied(MessageReplied),
}

impl MessageEvent {
    fn from_value(raw: &Value) -> Result<Self, serde_json::Error> {
        let subtype = raw
            .get("subtype")
            .and_then(Value::as_str)
            .map(MessageSubtype::from);
        Ok(match subtype {
            Some(MessageSubtype::MessageChanged) => {
                Self::Changed(MessageChanged::deserialize(raw)?)
            }
            Some(MessageSubtype::MessageDeleted) => {
                Self::Deleted(MessageDeleted::deserialize(raw)?)
            }
            Some(MessageSubtype::MessageReplied) => {
                Self::Replied(MessageReplied::deserialize(raw)?)
            }
            _ => Self::Message(Box::new(Message::deserialize(raw)?)),
        })
    }

    /// The conversation the event belongs to, when the payload carries it.
    pub fn channel(&self) -> Option<&ChannelId> {
        match self {
            Self::Message(m) => m.channel.as_ref(),
            Self::Changed(e) => e.channel.as_ref(),
            Self::Deleted(e) => e.channel.as_ref(),
            Self::Replied(e) => e.channel.as_ref(),
        }
    }
}

impl<'de> Deserialize<'de> for MessageEvent {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = Value::deserialize(d)?;
        Self::from_value(&raw).map_err(serde::de::Error::custom)
    }
}

impl Serialize for MessageEvent {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Message(m) => m.serialize(s),
            Self::Changed(e) => serialize_tagged("subtype", "message_changed", e, s),
            Self::Deleted(e) => serialize_tagged("subtype", "message_deleted", e, s),
            Self::Replied(e) => serialize_tagged("subtype", "message_replied", e, s),
        }
    }
}

/// `message_changed`: an edit or an unfurl applied to an existing message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageChanged {
    #[serde(default)]
    pub channel: Option<ChannelId>,
    /// The new state; `message.ts` identifies the edited message.
    pub message: Box<Message>,
    #[serde(default)]
    pub previous_message: Option<Box<Message>>,
    #[serde(default)]
    pub ts: Option<Ts>,
    #[serde(default)]
    pub event_ts: Option<Ts>,
}

/// `message_deleted`: `deleted_ts` is the removed message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageDeleted {
    #[serde(default)]
    pub channel: Option<ChannelId>,
    pub deleted_ts: Ts,
    #[serde(default)]
    pub previous_message: Option<Box<Message>>,
    #[serde(default)]
    pub ts: Option<Ts>,
    #[serde(default)]
    pub event_ts: Option<Ts>,
}

/// `message_replied`: thread bookkeeping on a root (`message` carries the updated root).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageReplied {
    #[serde(default)]
    pub channel: Option<ChannelId>,
    #[serde(default)]
    pub message: Option<Box<Message>>,
    #[serde(default)]
    pub ts: Option<Ts>,
    #[serde(default)]
    pub event_ts: Option<Ts>,
}

/// `reaction_added` / `reaction_removed`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReactionEvent {
    pub user: UserId,
    /// Shortcode without colons.
    pub reaction: String,
    #[serde(default, deserialize_with = "null_default")]
    pub item: ItemRef,
    #[serde(default)]
    pub item_user: Option<UserId>,
    #[serde(default)]
    pub event_ts: Option<Ts>,
}

/// What a reaction targets: a message (`channel` + `ts`) or a file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemRef {
    /// `message`, `file` or `file_comment`.
    #[serde(default, deserialize_with = "null_default")]
    pub r#type: String,
    #[serde(default)]
    pub channel: Option<ChannelId>,
    #[serde(default)]
    pub ts: Option<Ts>,
    #[serde(default)]
    pub file: Option<FileId>,
    #[serde(default)]
    pub file_comment: Option<String>,
}

/// `pin_added` / `pin_removed`. Note the channel key is `channel_id` here.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PinEvent {
    #[serde(default)]
    pub user: Option<UserId>,
    #[serde(default)]
    pub channel_id: Option<ChannelId>,
    /// Pinned item (`{type, message|file, ...}`), kept raw.
    #[serde(default)]
    pub item: Value,
    #[serde(default)]
    pub has_pins: Option<bool>,
    #[serde(default)]
    pub event_ts: Option<Ts>,
}

/// `star_added` / `star_removed` (saved items).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StarEvent {
    #[serde(default)]
    pub user: Option<UserId>,
    /// Starred item, kept raw.
    #[serde(default)]
    pub item: Value,
    #[serde(default)]
    pub event_ts: Option<Ts>,
}

/// `channel_marked` / `im_marked` / `group_marked` / `mpim_marked`: read cursor moved to `ts`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkedEvent {
    pub channel: ChannelId,
    pub ts: Ts,
    #[serde(default)]
    pub unread_count_display: Option<u32>,
    #[serde(default)]
    pub mention_count: Option<u32>,
    #[serde(default)]
    pub event_ts: Option<Ts>,
}

/// `thread_marked`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadMarkedEvent {
    pub subscription: ThreadSubscription,
    #[serde(default)]
    pub event_ts: Option<Ts>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadSubscription {
    pub channel: ChannelId,
    pub thread_ts: Ts,
    #[serde(default)]
    pub last_read: Option<Ts>,
    #[serde(default, deserialize_with = "null_default")]
    pub active: bool,
}

/// `user_typing` (5 s TTL on the receiving side).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserTyping {
    pub channel: ChannelId,
    pub user: UserId,
    #[serde(default)]
    pub thread_ts: Option<Ts>,
}

/// `presence_change`: single (`user`) or batched (`users`, with `batch_presence_aware=1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceChange {
    pub presence: Presence,
    #[serde(default)]
    pub user: Option<UserId>,
    #[serde(default, deserialize_with = "null_default")]
    pub users: Vec<UserId>,
}

impl PresenceChange {
    /// Every affected user, whichever shape arrived.
    pub fn user_ids(&self) -> impl Iterator<Item = &UserId> {
        self.user.iter().chain(self.users.iter())
    }
}

/// `manual_presence_change`: your own away toggle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManualPresenceChange {
    pub presence: Presence,
}

/// `dnd_updated` (you) / `dnd_updated_user` (someone else).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DndUpdated {
    #[serde(default)]
    pub user: Option<UserId>,
    #[serde(default, deserialize_with = "null_default")]
    pub dnd_status: DndStatus,
}

/// Events carrying a full (possibly partial) conversation object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelObjectEvent {
    pub channel: Box<Conversation>,
    #[serde(default)]
    pub event_ts: Option<Ts>,
}

/// Events carrying only a conversation id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelRefEvent {
    pub channel: ChannelId,
    #[serde(default)]
    pub user: Option<UserId>,
    #[serde(default)]
    pub event_ts: Option<Ts>,
}

/// `member_joined_channel` / `member_left_channel`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberChannelEvent {
    pub channel: ChannelId,
    pub user: UserId,
    /// `C` (public) or `G` (private).
    #[serde(default)]
    pub channel_type: Option<String>,
    #[serde(default)]
    pub team: Option<TeamId>,
    #[serde(default)]
    pub inviter: Option<UserId>,
    #[serde(default)]
    pub event_ts: Option<Ts>,
}

/// `user_change` / `user_profile_changed`: full updated user object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserChange {
    pub user: Box<User>,
    #[serde(default)]
    pub cache_ts: Option<i64>,
    #[serde(default)]
    pub event_ts: Option<Ts>,
}

/// `emoji_changed`: `subtype` is `add` (`name` + `value`), `remove` (`names`) or `rename`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmojiChanged {
    #[serde(default, deserialize_with = "null_default")]
    pub subtype: String,
    #[serde(default)]
    pub name: Option<String>,
    /// Image URL or `alias:<name>`.
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub names: Vec<String>,
    #[serde(default)]
    pub old_name: Option<String>,
    #[serde(default)]
    pub new_name: Option<String>,
    #[serde(default)]
    pub event_ts: Option<Ts>,
}

/// `pref_change`: one of your prefs; `value` shape varies per pref.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PrefChange {
    pub name: String,
    #[serde(default)]
    pub value: Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> SlackEvent {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn unknown_event_type_is_preserved() {
        let ev = parse(r#"{"type": "brand_new_event", "foo": {"bar": 1}, "event_ts": "1.2"}"#);
        match &ev {
            SlackEvent::Unknown { r#type, raw } => {
                assert_eq!(r#type, "brand_new_event");
                assert_eq!(raw["foo"]["bar"], 1);
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
        assert_eq!(ev.type_str(), "brand_new_event");
        // Serializes back unchanged.
        assert_eq!(
            serde_json::to_value(&ev).unwrap(),
            serde_json::json!({"type": "brand_new_event", "foo": {"bar": 1}, "event_ts": "1.2"})
        );
    }

    #[test]
    fn missing_type_and_drifted_payload_fall_back_to_unknown() {
        assert!(
            matches!(parse(r#"{"foo": 1}"#), SlackEvent::Unknown { r#type, .. } if r#type.is_empty())
        );
        // Known type, but `channel` became an object: never an error.
        assert!(matches!(
            parse(r#"{"type": "user_typing", "channel": {"id": "C1"}, "user": "U1"}"#),
            SlackEvent::Unknown { r#type, .. } if r#type == "user_typing"
        ));
    }

    #[test]
    fn message_with_unknown_subtype_is_a_message() {
        let ev = parse(
            r#"{"type": "message", "subtype": "brand_new_subtype", "channel": "C1",
                "user": "U1", "text": "hi", "ts": "1700000000.000100"}"#,
        );
        let SlackEvent::Message(MessageEvent::Message(m)) = ev else {
            panic!("expected message, got {ev:?}");
        };
        assert_eq!(
            m.subtype,
            Some(MessageSubtype::Other("brand_new_subtype".into()))
        );
        assert_eq!(m.channel.as_ref().map(ChannelId::as_str), Some("C1"));
    }

    #[test]
    fn message_subtypes_dispatch() {
        let changed = parse(
            r#"{"type": "message", "subtype": "message_changed", "hidden": true, "channel": "C1",
                "ts": "1700000002.000000",
                "message": {"type": "message", "user": "U1", "text": "new", "ts": "1700000000.000100",
                            "edited": {"user": "U1", "ts": "1700000002.000000"}},
                "previous_message": {"type": "message", "user": "U1", "text": "old", "ts": "1700000000.000100"}}"#,
        );
        let SlackEvent::Message(MessageEvent::Changed(c)) = &changed else {
            panic!("expected changed, got {changed:?}");
        };
        assert_eq!(c.message.text, "new");
        assert_eq!(c.previous_message.as_ref().unwrap().text, "old");

        let deleted = parse(
            r#"{"type": "message", "subtype": "message_deleted", "hidden": true, "channel": "C1",
                "ts": "1700000003.000000", "deleted_ts": "1700000000.000100"}"#,
        );
        assert!(matches!(
            &deleted,
            SlackEvent::Message(MessageEvent::Deleted(d)) if d.deleted_ts.as_str() == "1700000000.000100"
        ));

        let replied = parse(
            r#"{"type": "message", "subtype": "message_replied", "channel": "C1", "ts": "1700000004.000000",
                "message": {"type": "message", "user": "U1", "text": "root", "ts": "1700000000.000100",
                            "thread_ts": "1700000000.000100", "reply_count": 3, "latest_reply": "1700000004.000000"}}"#,
        );
        let SlackEvent::Message(MessageEvent::Replied(r)) = &replied else {
            panic!("expected replied, got {replied:?}");
        };
        assert_eq!(r.message.as_ref().unwrap().reply_count, Some(3));

        let broadcast = parse(
            r#"{"type": "message", "subtype": "thread_broadcast", "channel": "C1", "user": "U2",
                "text": "also here", "ts": "1700000005.000000", "thread_ts": "1700000000.000100",
                "root": {"user": "U1", "text": "root", "ts": "1700000000.000100"}}"#,
        );
        let SlackEvent::Message(MessageEvent::Message(b)) = &broadcast else {
            panic!("expected message, got {broadcast:?}");
        };
        assert_eq!(b.subtype, Some(MessageSubtype::ThreadBroadcast));
        assert!(b.is_thread_reply());
        assert_eq!(b.root.as_ref().unwrap().text, "root");

        // Round trip keeps the subtype tags.
        let json = serde_json::to_value(&changed).unwrap();
        assert_eq!(json["type"], "message");
        assert_eq!(json["subtype"], "message_changed");
        assert_eq!(serde_json::from_value::<SlackEvent>(json).unwrap(), changed);
    }

    #[test]
    fn parses_core_event_catalog() {
        let cases = [
            (r#"{"type":"hello","fast_reconnect":true}"#, "hello"),
            (r#"{"type":"goodbye"}"#, "goodbye"),
            (
                r#"{"type":"reconnect_url","url":"wss://x"}"#,
                "reconnect_url",
            ),
            (r#"{"type":"pong","reply_to":7}"#, "pong"),
            (
                r#"{"type":"error","error":{"code":1,"msg":"Socket URL has expired"}}"#,
                "error",
            ),
            (
                r#"{"type":"reaction_added","user":"U1","reaction":"eyes","item_user":"U2",
                   "item":{"type":"message","channel":"C1","ts":"1.1"},"event_ts":"2.2"}"#,
                "reaction_added",
            ),
            (
                r#"{"type":"reaction_removed","user":"U1","reaction":"eyes",
                   "item":{"type":"file","file":"F1"}}"#,
                "reaction_removed",
            ),
            (
                r#"{"type":"channel_marked","channel":"C1","ts":"1.1","event_ts":"2"}"#,
                "channel_marked",
            ),
            (
                r#"{"type":"im_marked","channel":"D1","ts":"1.1"}"#,
                "im_marked",
            ),
            (
                r#"{"type":"group_marked","channel":"G1","ts":"1.1"}"#,
                "group_marked",
            ),
            (
                r#"{"type":"mpim_marked","channel":"G2","ts":"1.1"}"#,
                "mpim_marked",
            ),
            (
                r#"{"type":"thread_marked","subscription":{"type":"thread","channel":"C1",
                   "thread_ts":"1.1","last_read":"1.2","active":true}}"#,
                "thread_marked",
            ),
            (
                r#"{"type":"user_typing","channel":"C1","user":"U1"}"#,
                "user_typing",
            ),
            (
                r#"{"type":"presence_change","user":"U1","presence":"active"}"#,
                "presence_change",
            ),
            (
                r#"{"type":"presence_change","users":["U1","U2"],"presence":"away"}"#,
                "presence_change",
            ),
            (
                r#"{"type":"manual_presence_change","presence":"away"}"#,
                "manual_presence_change",
            ),
            (
                r#"{"type":"channel_created","channel":{"id":"C9","is_channel":true,"name":"new",
                   "created":1700000000,"creator":"U1"},"event_ts":"1"}"#,
                "channel_created",
            ),
            (
                r#"{"type":"channel_rename","channel":{"id":"C9","name":"renamed","created":1}}"#,
                "channel_rename",
            ),
            (
                r#"{"type":"channel_archive","channel":"C9","user":"U1"}"#,
                "channel_archive",
            ),
            (
                r#"{"type":"member_joined_channel","user":"U1","channel":"C1","channel_type":"C",
                   "team":"T1","inviter":"U2"}"#,
                "member_joined_channel",
            ),
            (
                r#"{"type":"pin_added","user":"U1","channel_id":"C1","item":{"type":"message"},"has_pins":true}"#,
                "pin_added",
            ),
            (
                r#"{"type":"star_added","user":"U1","item":{"type":"message"}}"#,
                "star_added",
            ),
            (
                r#"{"type":"dnd_updated","user":"U1","dnd_status":{"dnd_enabled":true,
                   "next_dnd_start_ts":1,"next_dnd_end_ts":2,"snooze_enabled":false}}"#,
                "dnd_updated",
            ),
            (
                r#"{"type":"user_change","user":{"id":"U1","name":"egon","real_name":"Egon",
                   "profile":{"display_name":"spengler","status_text":"Print is dead",
                   "status_emoji":":books:","image_72":"https://a/72.png","title":"Doctor"},
                   "is_bot":false,"deleted":false,"tz":"America/New_York","updated":1502138686},
                   "cache_ts":1700000000}"#,
                "user_change",
            ),
            (
                r#"{"type":"emoji_changed","subtype":"add","name":"parrot","value":"https://e/p.gif"}"#,
                "emoji_changed",
            ),
            (
                r#"{"type":"emoji_changed","subtype":"remove","names":["parrot"]}"#,
                "emoji_changed",
            ),
            (
                r#"{"type":"pref_change","name":"muted_channels","value":"C1,C2"}"#,
                "pref_change",
            ),
        ];
        for (json, ty) in cases {
            let ev = parse(json);
            assert!(
                !matches!(ev, SlackEvent::Unknown { .. }),
                "{ty} fell back to Unknown: {ev:?}"
            );
            assert_eq!(ev.type_str(), ty);
        }

        let SlackEvent::UserChange(u) = parse(cases[23].0) else {
            panic!("expected user_change");
        };
        assert_eq!(u.user.display_name(), "spengler");
        assert_eq!(u.user.profile.status_emoji.as_deref(), Some(":books:"));

        let SlackEvent::PresenceChange(p) = parse(cases[14].0) else {
            panic!("expected presence_change");
        };
        assert_eq!(p.presence, Presence::Away);
        assert_eq!(p.user_ids().count(), 2);
    }
}
