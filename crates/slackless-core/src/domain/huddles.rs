//! Shared Huddle domain helpers: `huddle_thread` room-object decoder and live caption
//! deduplication (shared by `core` and CLI).

use serde_json::Value;

use crate::types::{Message, UserId};

/// Parsed metadata from a `huddle_thread` trail message's `extra["room"]` object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedHuddleRoom {
    pub live: bool,
    pub participants: Vec<UserId>,
    pub duration_secs: Option<u64>,
    pub topic: Option<String>,
    pub call_id: Option<String>,
}

/// Decode `message.extra["room"]` from a `huddle_thread` trail message.
pub fn parse_huddle_room(message: &Message) -> Option<ParsedHuddleRoom> {
    let room = message.extra.get("room")?;
    let has_active_participants = room
        .get("participants")
        .and_then(Value::as_array)
        .is_some_and(|ids| !ids.is_empty());
    let live = room
        .get("has_ended")
        .and_then(Value::as_bool)
        .is_some_and(|ended| !ended)
        && has_active_participants;
    let participants = room
        .get(if live {
            "participants"
        } else {
            "participant_history"
        })
        .or_else(|| room.get("participants"))
        .and_then(Value::as_array)
        .map_or_else(Vec::new, |ids| {
            ids.iter()
                .filter_map(Value::as_str)
                .map(UserId::from)
                .collect()
        });
    let duration_secs = (|| {
        let start = room.get("date_start")?.as_u64()?;
        let end = room.get("date_end")?.as_u64()?;
        (end > start).then_some(end - start)
    })();
    let topic = room
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    let call_id = room
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    Some(ParsedHuddleRoom {
        live,
        participants,
        duration_secs,
        topic,
        call_id,
    })
}

/// Deduplication window (milliseconds) for finalized huddle captions from the same speaker.
pub const CAPTION_DEDUP_WINDOW_MS: u64 = 2_500;

/// Stateful filter that trims finalized captions and suppresses duplicate speaker/text pairs
/// arriving within [`CAPTION_DEDUP_WINDOW_MS`].
#[derive(Debug, Default)]
pub struct CaptionDeduper {
    last: Option<(UserId, String, u64)>,
}

impl CaptionDeduper {
    /// Returns `Some(trimmed_text)` if this caption should be persisted, or `None` if it is
    /// partial, empty, or a duplicate within the 2.5s window.
    pub fn accept(
        &mut self,
        user: &UserId,
        text: &str,
        is_partial: bool,
        at_ms: u64,
    ) -> Option<String> {
        if is_partial {
            return None;
        }
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        let is_dup = self.last.as_ref().is_some_and(|(u, t, ts)| {
            u == user && t == trimmed && at_ms.saturating_sub(*ts) < CAPTION_DEDUP_WINDOW_MS
        });
        if is_dup {
            return None;
        }
        let owned = trimmed.to_owned();
        self.last = Some((user.clone(), owned.clone(), at_ms));
        Some(owned)
    }
}

/// Resolve a caption timestamp to epoch milliseconds (`i64`), falling back to wall-clock time
/// when `at_ms == 0`.
pub fn resolve_segment_at_ms(at_ms: u64) -> i64 {
    if at_ms > 0 {
        i64::try_from(at_ms).unwrap_or(i64::MAX)
    } else {
        i64::try_from(crate::types::time::now_epoch_ms()).unwrap_or(i64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_huddle_room_decodes_live_and_ended_rooms() {
        let mut live_msg = Message::default();
        live_msg.extra.insert(
            "room".into(),
            json!({
                "id": "R123",
                "name": "Standup",
                "has_ended": false,
                "participants": ["U1", "U2"],
                "date_start": 1000
            }),
        );
        let parsed = parse_huddle_room(&live_msg).expect("live room");
        assert!(parsed.live);
        assert_eq!(
            parsed.participants,
            vec![UserId::from("U1"), UserId::from("U2")]
        );
        assert_eq!(parsed.duration_secs, None);
        assert_eq!(parsed.topic.as_deref(), Some("Standup"));
        assert_eq!(parsed.call_id.as_deref(), Some("R123"));

        let mut ended_msg = Message::default();
        ended_msg.extra.insert(
            "room".into(),
            json!({
                "has_ended": true,
                "participants": [],
                "participant_history": ["U1", "U3"],
                "date_start": 1000,
                "date_end": 1600
            }),
        );
        let ended = parse_huddle_room(&ended_msg).expect("ended room");
        assert!(!ended.live);
        assert_eq!(
            ended.participants,
            vec![UserId::from("U1"), UserId::from("U3")]
        );
        assert_eq!(ended.duration_secs, Some(600));
    }

    #[test]
    fn caption_deduper_filters_partials_empty_and_2500ms_dups() {
        let mut dedup = CaptionDeduper::default();
        let u1 = UserId::from("U1");
        let u2 = UserId::from("U2");

        assert_eq!(dedup.accept(&u1, "hello", true, 1000), None);
        assert_eq!(dedup.accept(&u1, "   ", false, 1000), None);
        assert_eq!(
            dedup.accept(&u1, "  hello  ", false, 1000),
            Some("hello".into())
        );
        // Same speaker + text within 2.5s -> suppressed.
        assert_eq!(dedup.accept(&u1, "hello", false, 3499), None);
        // At >= 2.5s -> accepted.
        assert_eq!(
            dedup.accept(&u1, "hello", false, 3500),
            Some("hello".into())
        );
        // Different speaker immediately -> accepted.
        assert_eq!(
            dedup.accept(&u2, "hello", false, 3501),
            Some("hello".into())
        );
    }
}
