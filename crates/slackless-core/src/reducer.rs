//! Pure websocket-event -> store reducer.

use crate::Event;
use crate::store::{self, Store};
use crate::types::event::{MessageEvent, ReactionEvent};
use crate::types::{ChannelId, Reaction, SlackEvent, TeamId, UserId};

/// Applies one websocket event to the store and says what changed.
///
/// `None` means nothing a front-end renders changed (control frames, events the store does not
/// model, and [`SlackEvent::Unknown`] types Slack added later). A store failure becomes
/// [`Event::Error`]. Ephemeral events (typing, presence, read markers) pass straight through.
pub fn apply_event(store: &mut Store, workspace: &TeamId, event: SlackEvent) -> Option<Event> {
    apply(store, workspace, event).unwrap_or_else(|e| Some(Event::Error(format!("store: {e}"))))
}

fn apply(store: &mut Store, ws: &TeamId, event: SlackEvent) -> store::Result<Option<Event>> {
    Ok(match event {
        SlackEvent::Message(ev) => apply_message(store, ws, ev)?,
        SlackEvent::ReactionAdded(r) => apply_reaction(store, ws, &r, true)?,
        SlackEvent::ReactionRemoved(r) => apply_reaction(store, ws, &r, false)?,
        SlackEvent::ChannelCreated(e)
        | SlackEvent::ChannelJoined(e)
        | SlackEvent::ChannelRename(e)
        | SlackEvent::GroupRename(e) => {
            store.upsert_conversation(ws, &e.channel)?;
            Some(Event::Conversations(vec![e.channel.id.clone()]))
        }
        SlackEvent::UserChange(e) | SlackEvent::UserProfileChanged(e) => {
            store.upsert_user(ws, &e.user)?;
            Some(Event::UsersChanged(vec![e.user.id.clone()]))
        }
        SlackEvent::ChannelMarked(m)
        | SlackEvent::ImMarked(m)
        | SlackEvent::GroupMarked(m)
        | SlackEvent::MpimMarked(m) => Some(Event::ReadMarker {
            channel: m.channel,
            ts: m.ts,
        }),
        SlackEvent::UserTyping(t) => Some(Event::Typing {
            channel: t.channel,
            user: t.user,
            thread_ts: t.thread_ts,
        }),
        SlackEvent::PresenceChange(p) => Some(Event::Presence {
            users: p.user_ids().cloned().collect(),
            presence: p.presence,
        }),
        _ => None,
    })
}

fn apply_message(store: &mut Store, ws: &TeamId, ev: MessageEvent) -> store::Result<Option<Event>> {
    let Some(channel) = ev.channel().cloned() else {
        return Ok(None);
    };
    match ev {
        MessageEvent::Message(m) => store.upsert_message(ws, &channel, &m)?,
        MessageEvent::Changed(c) => store.upsert_message(ws, &channel, &c.message)?,
        MessageEvent::Deleted(d) => store.record_deletion(
            ws,
            &channel,
            &d.deleted_ts,
            d.event_ts.as_ref().or(d.ts.as_ref()),
            d.previous_message.as_deref(),
        )?,
        MessageEvent::Replied(r) => match r.message {
            Some(root) => store.upsert_message(ws, &channel, &root)?,
            None => return Ok(None),
        },
    }
    Ok(Some(Event::StoreChanged { channel }))
}

/// Reactions live on the stored message: the current state is re-written with the reaction
/// toggled. Reactions on messages not in the store (or on files) are dropped; the next history
/// fetch carries them.
fn apply_reaction(
    store: &mut Store,
    ws: &TeamId,
    ev: &ReactionEvent,
    added: bool,
) -> store::Result<Option<Event>> {
    let (Some(channel), Some(ts)) = (&ev.item.channel, &ev.item.ts) else {
        return Ok(None);
    };
    let current = store
        .thread_replies(ws, channel, ts)?
        .into_iter()
        .find(|m| m.message.ts == *ts);
    let Some(current) = current.filter(|m| !m.deleted) else {
        return Ok(None);
    };
    let mut message = current.message;
    toggle_reaction(&mut message.reactions, &ev.reaction, &ev.user, added);
    store.upsert_message(ws, channel, &message)?;
    Ok(Some(Event::StoreChanged {
        channel: ChannelId::clone(channel),
    }))
}

fn toggle_reaction(reactions: &mut Vec<Reaction>, name: &str, user: &UserId, added: bool) {
    let index = reactions.iter().position(|r| r.name == name);
    match (index, added) {
        (Some(i), true) => {
            let r = &mut reactions[i];
            if !r.users.contains(user) {
                r.users.push(user.clone());
                r.count += 1;
            }
        }
        (Some(i), false) => {
            let r = &mut reactions[i];
            r.users.retain(|u| u != user);
            // `users` may be truncated on busy messages, so the count drops regardless.
            r.count = r.count.saturating_sub(1);
            if r.count == 0 {
                reactions.remove(i);
            }
        }
        (None, true) => reactions.push(Reaction {
            name: name.to_owned(),
            count: 1,
            users: vec![user.clone()],
        }),
        (None, false) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Ts;

    fn ws() -> TeamId {
        TeamId::from("T1")
    }

    fn c1() -> ChannelId {
        ChannelId::from("C1")
    }

    fn event(json: &str) -> SlackEvent {
        serde_json::from_str(json).unwrap()
    }

    fn history(store: &Store) -> Vec<store::StoredMessage> {
        store.channel_history(&ws(), &c1(), None, 50).unwrap()
    }

    const HELLO: &str = r#"{"type":"message","channel":"C1","user":"U1","text":"hello",
        "ts":"1700000000.000100"}"#;

    #[test]
    fn inbound_message_lands_in_store_and_emits_store_changed() {
        let mut store = Store::open_in_memory().unwrap();
        let out = apply_event(&mut store, &ws(), event(HELLO));
        assert_eq!(out, Some(Event::StoreChanged { channel: c1() }));
        let stored = history(&store);
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].message.text, "hello");
    }

    #[test]
    fn unknown_subtype_is_kept_and_unknown_event_is_ignored() {
        let mut store = Store::open_in_memory().unwrap();
        let out = apply_event(
            &mut store,
            &ws(),
            event(
                r#"{"type":"message","subtype":"brand_new","channel":"C1","text":"x","ts":"1.1"}"#,
            ),
        );
        assert_eq!(out, Some(Event::StoreChanged { channel: c1() }));
        assert_eq!(history(&store).len(), 1);
        assert_eq!(
            apply_event(&mut store, &ws(), event(r#"{"type":"brand_new_event"}"#)),
            None
        );
    }

    #[test]
    fn edit_and_delete_go_through_the_revision_log() {
        let mut store = Store::open_in_memory().unwrap();
        apply_event(&mut store, &ws(), event(HELLO));
        let changed = apply_event(
            &mut store,
            &ws(),
            event(
                r#"{"type":"message","subtype":"message_changed","channel":"C1","ts":"1700000005.0",
                    "message":{"user":"U1","text":"hello there","ts":"1700000000.000100",
                               "edited":{"user":"U1","ts":"1700000005.0"}}}"#,
            ),
        );
        assert_eq!(changed, Some(Event::StoreChanged { channel: c1() }));
        assert_eq!(history(&store)[0].message.text, "hello there");

        let deleted = apply_event(
            &mut store,
            &ws(),
            event(
                r#"{"type":"message","subtype":"message_deleted","channel":"C1",
                    "ts":"1700000009.0","deleted_ts":"1700000000.000100"}"#,
            ),
        );
        assert_eq!(deleted, Some(Event::StoreChanged { channel: c1() }));
        let stored = history(&store);
        assert!(stored[0].deleted);
        assert_eq!(
            stored[0].message.text, "hello there",
            "history never disappears"
        );
        let revisions = store
            .revisions(&ws(), &c1(), &Ts::from("1700000000.000100"))
            .unwrap();
        assert_eq!(revisions.len(), 3);
    }

    #[test]
    fn reactions_toggle_on_the_stored_message() {
        let mut store = Store::open_in_memory().unwrap();
        apply_event(&mut store, &ws(), event(HELLO));
        let reaction = |kind: &str, user: &str| {
            event(&format!(
                r#"{{"type":"{kind}","user":"{user}","reaction":"eyes",
                    "item":{{"type":"message","channel":"C1","ts":"1700000000.000100"}}}}"#
            ))
        };
        let out = apply_event(&mut store, &ws(), reaction("reaction_added", "U2"));
        assert_eq!(out, Some(Event::StoreChanged { channel: c1() }));
        apply_event(&mut store, &ws(), reaction("reaction_added", "U3"));
        let r = &history(&store)[0].message.reactions;
        assert_eq!(
            (r[0].name.as_str(), r[0].count, r[0].users.len()),
            ("eyes", 2, 2)
        );

        apply_event(&mut store, &ws(), reaction("reaction_removed", "U2"));
        apply_event(&mut store, &ws(), reaction("reaction_removed", "U3"));
        assert!(history(&store)[0].message.reactions.is_empty());

        // Unknown message: nothing to update.
        let stray = event(
            r#"{"type":"reaction_added","user":"U2","reaction":"eyes",
                "item":{"type":"message","channel":"C1","ts":"9.9"}}"#,
        );
        assert_eq!(apply_event(&mut store, &ws(), stray), None);
    }

    #[test]
    fn conversation_and_ephemeral_events() {
        let mut store = Store::open_in_memory().unwrap();
        let created = apply_event(
            &mut store,
            &ws(),
            event(
                r#"{"type":"channel_created","channel":{"id":"C9","name":"new","is_channel":true}}"#,
            ),
        );
        assert_eq!(
            created,
            Some(Event::Conversations(vec![ChannelId::from("C9")]))
        );
        assert_eq!(
            apply_event(
                &mut store,
                &ws(),
                event(r#"{"type":"channel_marked","channel":"C1","ts":"1.5"}"#)
            ),
            Some(Event::ReadMarker {
                channel: c1(),
                ts: Ts::from("1.5")
            })
        );
        assert_eq!(
            apply_event(&mut store, &ws(), event(r#"{"type":"hello"}"#)),
            None
        );
    }
}
