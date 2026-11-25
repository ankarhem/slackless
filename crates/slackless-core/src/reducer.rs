//! Pure websocket-event -> store reducer.

use crate::Event;
use crate::store::{self, Store};
use crate::types::event::{MessageEvent, ReactionEvent};
use crate::types::{ChannelId, Reaction, SlackEvent, TeamId, Ts, UserId};

/// Update a stored conversation's `last_read` (and optionally `unread_count_display`) without
/// clobbering other metadata. Shared across RTM read-marker events, online `mark_read`, and
/// offline queue flush.
pub(crate) fn update_conversation_last_read(
    store: &mut Store,
    ws: &TeamId,
    channel: &ChannelId,
    ts: Ts,
    unread_count: Option<u32>,
) -> store::Result<()> {
    if let Some(mut conv) = store.conversation(ws, channel)? {
        conv.last_read = Some(ts);
        if let Some(unread) = unread_count {
            conv.unread_count_display = Some(unread);
        }
        store.upsert_conversation(ws, &conv)?;
    }
    Ok(())
}

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
            store.merge_conversation(ws, &e.channel)?;
            Some(Event::Conversations(vec![e.channel.id.clone()]))
        }
        SlackEvent::UserChange(e)
        | SlackEvent::UserProfileChanged(e)
        | SlackEvent::UserHuddleChanged(e) => {
            store.upsert_user(ws, &e.user)?;
            Some(Event::UsersChanged(vec![e.user.id.clone()]))
        }
        SlackEvent::ChannelMarked(m)
        | SlackEvent::ImMarked(m)
        | SlackEvent::GroupMarked(m)
        | SlackEvent::MpimMarked(m) => {
            let unread = m
                .unread_count_display
                .or_else(|| (m.dm_count == Some(0)).then_some(0));
            update_conversation_last_read(store, ws, &m.channel, m.ts.clone(), unread)?;
            Some(Event::ReadMarker {
                channel: m.channel,
                ts: m.ts,
            })
        }
        SlackEvent::ThreadMarked(m) => Some(Event::ThreadReadMarker {
            channel: m.subscription.channel,
            thread_ts: m.subscription.thread_ts,
            last_read: m.subscription.last_read,
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
    let current = store.message(ws, channel, ts)?;
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
            apply_event(
                &mut store,
                &ws(),
                event(
                    r#"{"type":"thread_marked","subscription":{"channel":"C1","thread_ts":"1.0","last_read":"1.9","active":true}}"#
                )
            ),
            Some(Event::ThreadReadMarker {
                channel: c1(),
                thread_ts: Ts::from("1.0"),
                last_read: Some(Ts::from("1.9")),
            })
        );
        assert_eq!(
            apply_event(&mut store, &ws(), event(r#"{"type":"hello"}"#)),
            None
        );
    }

    #[test]
    fn im_marked_persists_last_read_and_unread_count() {
        let mut store = Store::open_in_memory().unwrap();
        let dm = crate::types::Conversation {
            id: ChannelId::from("D1"),
            is_im: true,
            last_read: Some(Ts::from("1.0")),
            unread_count_display: Some(3),
            ..Default::default()
        };
        store.upsert_conversation(&ws(), &dm).unwrap();
        let ev = apply_event(
            &mut store,
            &ws(),
            event(r#"{"type":"im_marked","channel":"D1","ts":"1.5","dm_count":0}"#),
        );
        assert_eq!(
            ev,
            Some(Event::ReadMarker {
                channel: ChannelId::from("D1"),
                ts: Ts::from("1.5"),
            })
        );
        let stored = store
            .conversation(&ws(), &ChannelId::from("D1"))
            .unwrap()
            .unwrap();
        assert_eq!(stored.last_read, Some(Ts::from("1.5")));
        assert_eq!(stored.unread_count_display, Some(0));
    }

    #[test]
    fn replied_and_missing_channel_message_events() {
        let mut store = Store::open_in_memory().unwrap();
        // Missing channel is ignored.
        let orphan = apply_event(
            &mut store,
            &ws(),
            event(r#"{"type":"message","user":"U1","text":"orphan","ts":"1.0"}"#),
        );
        assert_eq!(orphan, None);

        // `message_replied` with `message` upserts the thread root.
        let replied = apply_event(
            &mut store,
            &ws(),
            event(
                r#"{"type":"message","subtype":"message_replied","channel":"C1",
                    "message":{"user":"U1","text":"thread root","ts":"10.0","reply_count":2}}"#,
            ),
        );
        assert_eq!(replied, Some(Event::StoreChanged { channel: c1() }));
        assert_eq!(history(&store)[0].message.text, "thread root");
        assert_eq!(history(&store)[0].message.reply_count, Some(2));

        // `message_replied` without `message` payload is a no-op.
        let empty_replied = apply_event(
            &mut store,
            &ws(),
            event(r#"{"type":"message","subtype":"message_replied","channel":"C1"}"#),
        );
        assert_eq!(empty_replied, None);
    }

    #[test]
    fn deleted_with_previous_message_on_unseen_row_preserves_text() {
        let mut store = Store::open_in_memory().unwrap();
        let out = apply_event(
            &mut store,
            &ws(),
            event(
                r#"{"type":"message","subtype":"message_deleted","channel":"C1",
                    "deleted_ts":"20.0","event_ts":"21.0",
                    "previous_message":{"user":"U1","text":"vanished before sync","ts":"20.0"}}"#,
            ),
        );
        assert_eq!(out, Some(Event::StoreChanged { channel: c1() }));
        let stored = history(&store);
        assert_eq!(stored.len(), 1);
        assert!(stored[0].deleted);
        assert_eq!(stored[0].message.text, "vanished before sync");
    }

    #[test]
    fn reaction_edge_cases_deleted_duplicate_and_truncated_users() {
        let mut store = Store::open_in_memory().unwrap();
        // Seed a message with a reaction whose `users` list is truncated (count 3, only U1 listed).
        store
            .upsert_message(
                &ws(),
                &c1(),
                &crate::types::Message {
                    ts: Ts::from("30.0"),
                    text: "popular".into(),
                    reactions: vec![Reaction {
                        name: "fire".into(),
                        count: 3,
                        users: vec![UserId::from("U1")],
                    }],
                    ..Default::default()
                },
            )
            .unwrap();

        // Duplicate `reaction_added` from U1 does not bump count or duplicate user.
        apply_event(
            &mut store,
            &ws(),
            event(
                r#"{"type":"reaction_added","user":"U1","reaction":"fire",
                    "item":{"type":"message","channel":"C1","ts":"30.0"}}"#,
            ),
        );
        let r = &history(&store)[0].message.reactions[0];
        assert_eq!((r.count, r.users.len()), (3, 1));

        // `reaction_removed` for U99 (absent from truncated `users`) still decrements count.
        apply_event(
            &mut store,
            &ws(),
            event(
                r#"{"type":"reaction_removed","user":"U99","reaction":"fire",
                    "item":{"type":"message","channel":"C1","ts":"30.0"}}"#,
            ),
        );
        let r = &history(&store)[0].message.reactions[0];
        assert_eq!((r.count, r.users.len()), (2, 1));

        // Delete the message; subsequent reactions on a deleted message return None.
        apply_event(
            &mut store,
            &ws(),
            event(
                r#"{"type":"message","subtype":"message_deleted","channel":"C1",
                    "deleted_ts":"30.0","ts":"31.0"}"#,
            ),
        );
        let on_deleted = apply_event(
            &mut store,
            &ws(),
            event(
                r#"{"type":"reaction_added","user":"U2","reaction":"eyes",
                    "item":{"type":"message","channel":"C1","ts":"30.0"}}"#,
            ),
        );
        assert_eq!(on_deleted, None);
    }

    #[test]
    fn channel_user_marked_typing_and_presence_events_reduce_cleanly() {
        let mut store = Store::open_in_memory().unwrap();

        for json in [
            r#"{"type":"channel_joined","channel":{"id":"C2","name":"ops","is_channel":true,"is_member":true}}"#,
            r#"{"type":"channel_rename","channel":{"id":"C2","name":"ops-renamed","is_channel":true}}"#,
            r#"{"type":"group_rename","channel":{"id":"G2","name":"secret-renamed","is_group":true}}"#,
        ] {
            let ev = apply_event(&mut store, &ws(), event(json));
            assert!(matches!(ev, Some(Event::Conversations(ref ids)) if ids.len() == 1));
        }
        assert_eq!(
            store
                .conversation(&ws(), &ChannelId::from("C2"))
                .unwrap()
                .unwrap()
                .name
                .as_deref(),
            Some("ops-renamed")
        );

        let user_ev = apply_event(
            &mut store,
            &ws(),
            event(r#"{"type":"user_change","user":{"id":"U7","name":"alice"}}"#),
        );
        assert_eq!(user_ev, Some(Event::UsersChanged(vec![UserId::from("U7")])));
        assert!(
            store
                .users(&ws())
                .unwrap()
                .iter()
                .any(|u| u.id == UserId::from("U7"))
        );

        // Seed G2 and MPIM G3 so group_marked / mpim_marked update last_read & unread_count_display.
        for id in ["G2", "G3"] {
            store
                .upsert_conversation(
                    &ws(),
                    &crate::types::Conversation {
                        id: ChannelId::from(id),
                        is_group: id == "G2",
                        is_mpim: id == "G3",
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        assert_eq!(
            apply_event(
                &mut store,
                &ws(),
                event(
                    r#"{"type":"group_marked","channel":"G2","ts":"5.5","unread_count_display":1}"#
                )
            ),
            Some(Event::ReadMarker {
                channel: ChannelId::from("G2"),
                ts: Ts::from("5.5"),
            })
        );
        assert_eq!(
            store
                .conversation(&ws(), &ChannelId::from("G2"))
                .unwrap()
                .unwrap()
                .unread_count_display,
            Some(1)
        );
        assert_eq!(
            apply_event(
                &mut store,
                &ws(),
                event(r#"{"type":"mpim_marked","channel":"G3","ts":"6.6","dm_count":0}"#)
            ),
            Some(Event::ReadMarker {
                channel: ChannelId::from("G3"),
                ts: Ts::from("6.6"),
            })
        );
        assert_eq!(
            store
                .conversation(&ws(), &ChannelId::from("G3"))
                .unwrap()
                .unwrap()
                .unread_count_display,
            Some(0)
        );

        assert_eq!(
            apply_event(
                &mut store,
                &ws(),
                event(r#"{"type":"user_typing","channel":"C1","user":"U2","thread_ts":"1.0"}"#)
            ),
            Some(Event::Typing {
                channel: c1(),
                user: UserId::from("U2"),
                thread_ts: Some(Ts::from("1.0")),
            })
        );
        assert_eq!(
            apply_event(
                &mut store,
                &ws(),
                event(r#"{"type":"presence_change","users":["U2","U3"],"presence":"active"}"#)
            ),
            Some(Event::Presence {
                users: vec![UserId::from("U2"), UserId::from("U3")],
                presence: crate::types::Presence::Active,
            })
        );
    }

    #[test]
    fn channel_and_group_rename_preserve_existing_conversation_metadata() {
        let mut store = Store::open_in_memory().expect("in-memory store");
        let existing = crate::types::Conversation {
            id: ChannelId::from("C2"),
            name: Some("ops".into()),
            is_channel: true,
            is_member: true,
            topic: Some(crate::types::TopicPurpose {
                value: "On-call rotation".into(),
                ..Default::default()
            }),
            purpose: Some(crate::types::TopicPurpose {
                value: "Incident response".into(),
                ..Default::default()
            }),
            last_read: Some(Ts::from("1700.5")),
            properties: Some(crate::types::ConversationProperties {
                canvas: Some(crate::types::ChannelCanvas {
                    file_id: Some(crate::types::FileId::from("FCANV")),
                    is_empty: false,
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        store.upsert_conversation(&ws(), &existing).expect("seed");

        // Partial Slack `channel_rename` payload omits `is_member`, `topic`, `purpose`, `last_read`, `properties`.
        let ev = apply_event(
            &mut store,
            &ws(),
            event(
                r#"{"type":"channel_rename","channel":{"id":"C2","name":"ops-v2","is_channel":true}}"#,
            ),
        );
        assert_eq!(ev, Some(Event::Conversations(vec![ChannelId::from("C2")])));

        let stored = store
            .conversation(&ws(), &ChannelId::from("C2"))
            .expect("query")
            .expect("row");
        assert_eq!(stored.name.as_deref(), Some("ops-v2"));
        assert!(stored.is_member, "rename must preserve is_member");
        assert_eq!(
            stored.topic.as_ref().map(|t| t.value.as_str()),
            Some("On-call rotation")
        );
        assert_eq!(
            stored.purpose.as_ref().map(|p| p.value.as_str()),
            Some("Incident response")
        );
        assert_eq!(stored.last_read, Some(Ts::from("1700.5")));
        assert_eq!(
            stored
                .properties
                .as_ref()
                .and_then(|p| p.canvas.as_ref())
                .and_then(|c| c.file_id.as_ref())
                .map(|id| id.as_str()),
            Some("FCANV")
        );
    }
}
