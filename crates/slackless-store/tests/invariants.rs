//! Seeded randomized state-machine & invariant tests for `slackless-store`.
//!
//! Verifies properties that point unit tests often miss:
//! - FTS5 index (`message_fts`) always matches the live non-deleted `message_current` rows across
//!   arbitrary interleavings of inserts, edits, deletes, and duplicate upserts.
//! - The immutable `message_revision` audit log is strictly monotonic (never loses history).
//! - The V13 durable outbox state machine preserves FIFO ordering, monotonic attempts, and clean
//!   crash recovery (`sending -> pending`).

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, HashMap};

use slackless_store::{NewOutboxItem, OutboxKind, OutboxStatus, SearchQuery, Store};
use slackless_types::{ChannelId, Conversation, Message, TeamId, Ts, UserId};

/// Tiny deterministic xorshift64 PRNG so randomized property tests are 100% reproducible without
/// adding external crate dependencies.
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
fn fts5_and_immutable_revisions_hold_invariants_under_randomized_mutations() {
    let mut store = Store::open_in_memory().unwrap();
    let ws = TeamId::from("T0INV");
    let channels = [ChannelId::from("C0A"), ChannelId::from("C0B")];
    for ch in &channels {
        store
            .upsert_conversation(
                &ws,
                &Conversation {
                    id: ch.clone(),
                    name: Some(ch.as_str().to_lowercase()),
                    is_channel: true,
                    is_member: true,
                    ..Default::default()
                },
            )
            .unwrap();
    }

    let vocab = ["alpha", "bravo", "charlie", "delta", "echo", "foxtrot"];
    let mut rng = Rng(0xDEAD_BEEF_CAFE_BABEu64);

    // Reference model: (channel, ts) -> (current_token, deleted, revision_count)
    let mut model: HashMap<(ChannelId, Ts), (String, bool, usize)> = HashMap::new();
    let mut next_seq = 1u64;

    for step in 0..250usize {
        let op = rng.usize(10);
        if model.is_empty() || op < 4 {
            // Insert a brand-new message.
            let ch = channels[rng.usize(channels.len())].clone();
            let ts = Ts::from(format!("1700000{:03}.000100", next_seq));
            next_seq += 1;
            let token = format!("tok{}_{}", vocab[rng.usize(vocab.len())], step);
            let msg = Message {
                ts: ts.clone(),
                user: Some(UserId::from("U0DEV")),
                text: format!("payload {token}"),
                ..Default::default()
            };
            store.upsert_message(&ws, &ch, &msg).unwrap();
            model.insert((ch, ts), (token, false, 1));
        } else if op < 7 {
            // Edit an existing non-deleted message (or re-upsert unchanged to test idempotency).
            let keys: Vec<_> = model.keys().cloned().collect();
            let key = keys[rng.usize(keys.len())].clone();
            let (cur_tok, deleted, rev_count) = model.get_mut(&key).unwrap();
            if !*deleted {
                let new_tok = format!("tok{}_{}", vocab[rng.usize(vocab.len())], step);
                let mut msg = Message {
                    ts: key.1.clone(),
                    user: Some(UserId::from("U0DEV")),
                    text: format!("edited {new_tok}"),
                    ..Default::default()
                };
                msg.edited = Some(slackless_types::Edited {
                    user: Some(UserId::from("U0DEV")),
                    ts: Ts::from(format!("1700009{:03}.000100", step)),
                });
                store.upsert_message(&ws, &key.0, &msg).unwrap();
                *cur_tok = new_tok;
                *rev_count += 1;
            }
        } else if op < 9 {
            // Delete an existing message.
            let keys: Vec<_> = model.keys().cloned().collect();
            let key = keys[rng.usize(keys.len())].clone();
            let (_, deleted, rev_count) = model.get_mut(&key).unwrap();
            if !*deleted {
                let del_ts = Ts::from(format!("1700010{:03}.000100", step));
                store
                    .record_deletion(&ws, &key.0, &key.1, Some(&del_ts), None)
                    .unwrap();
                *deleted = true;
                *rev_count += 1;
            }
        } else {
            // Spot-check FTS5 & revision invariants mid-stream.
            for ((ch, ts), (tok, deleted, expected_revs)) in &model {
                let hits = store.search(&ws, &SearchQuery::parse(tok), 20).unwrap();
                let found = hits.iter().any(|h| h.channel == *ch && h.message.ts == *ts);
                assert_eq!(
                    found, !*deleted,
                    "FTS5 mismatch for ({ch}, {ts}) token={tok} deleted={deleted}"
                );
                let revs = store.revisions(&ws, ch, ts).unwrap();
                assert!(
                    revs.len() >= *expected_revs,
                    "immutable revision log shrank for ({ch}, {ts}): got {} < {}",
                    revs.len(),
                    expected_revs
                );
            }
        }
    }

    // Final exhaustive check across all modeled rows.
    for ((ch, ts), (tok, deleted, expected_revs)) in &model {
        let row = store.message(&ws, ch, ts).unwrap().expect("row exists");
        assert_eq!(row.deleted, *deleted);
        let hits = store.search(&ws, &SearchQuery::parse(tok), 20).unwrap();
        let found = hits.iter().any(|h| h.channel == *ch && h.message.ts == *ts);
        assert_eq!(found, !*deleted);
        let revs = store.revisions(&ws, ch, ts).unwrap();
        assert!(revs.len() >= *expected_revs);
    }
}

#[test]
fn durable_outbox_state_machine_preserves_fifo_attempts_and_crash_recovery() {
    let mut store = Store::open_in_memory().unwrap();
    let ws = TeamId::from("T0OUTBOX");
    let ch = ChannelId::from("C0ENG");
    let mut rng = Rng(0x1234_5678_9ABC_DEF0u64);

    // Reference model keyed by row id -> (status, attempts).
    let mut model: BTreeMap<i64, (OutboxStatus, u32)> = BTreeMap::new();

    for step in 0..160i64 {
        match rng.usize(6) {
            0 | 1 => {
                // Enqueue a new pending item.
                let payload = format!(r#"{{"text":"msg-{step}"}}"#);
                let item = store
                    .enqueue_outbox(
                        &ws,
                        &NewOutboxItem {
                            channel_id: &ch,
                            thread_ts: None,
                            kind: OutboxKind::Message,
                            payload_json: &payload,
                            last_error: None,
                        },
                    )
                    .unwrap();
                assert_eq!(item.status, OutboxStatus::Pending);
                assert_eq!(item.attempts, 0);
                model.insert(item.id, (OutboxStatus::Pending, 0));
            }
            2 => {
                // Transition the oldest Pending item to Sending.
                if let Some((&id, (status, attempts))) = model
                    .iter_mut()
                    .find(|(_, (st, _))| *st == OutboxStatus::Pending)
                {
                    let claimed = store.mark_outbox_sending(id).unwrap();
                    assert!(claimed);
                    *status = OutboxStatus::Sending;
                    *attempts += 1;
                }
            }
            3 => {
                // Transition a Sending item to either Failed or Sent (deleted).
                if let Some((&id, (status, _))) = model
                    .iter_mut()
                    .find(|(_, (st, _))| *st == OutboxStatus::Sending)
                {
                    if rng.usize(2) == 0 {
                        store.mark_outbox_failed(id, "network timeout").unwrap();
                        *status = OutboxStatus::Failed;
                    } else {
                        store.delete_outbox_item(id).unwrap();
                        model.remove(&id);
                    }
                }
            }
            4 => {
                // Retry a Failed item back to Pending.
                if let Some((&id, (status, _))) = model
                    .iter_mut()
                    .find(|(_, (st, _))| *st == OutboxStatus::Failed)
                {
                    store.reset_outbox_item_pending(id).unwrap();
                    *status = OutboxStatus::Pending;
                }
            }
            _ => {
                // Simulate crash recovery: every Sending row must reset to Pending without losing attempts.
                store.reset_outbox_sending_to_pending(&ws).unwrap();
                for (st, _) in model.values_mut() {
                    if *st == OutboxStatus::Sending {
                        *st = OutboxStatus::Pending;
                    }
                }
            }
        }

        // Assert exact FIFO order, status, and attempt count against the reference model.
        let rows = store.list_outbox(&ws).unwrap();
        assert_eq!(rows.len(), model.len());
        let pending_only = store.list_outbox_pending(&ws).unwrap();
        assert_eq!(
            pending_only.len(),
            model
                .values()
                .filter(|(st, _)| *st == OutboxStatus::Pending)
                .count()
        );
        for (row, (&id, &(expected_status, expected_attempts))) in rows.iter().zip(model.iter()) {
            assert_eq!(row.id, id, "outbox rows must stay strictly FIFO by id");
            assert_eq!(row.status, expected_status);
            assert_eq!(row.attempts, expected_attempts);
        }
    }

    // Exercise channel/thread scoping, single-item lookup, and double-claim idempotency.
    let thread_root = Ts::from("1700000555.000100");
    let reply = store
        .enqueue_outbox(
            &ws,
            &NewOutboxItem {
                channel_id: &ch,
                thread_ts: Some(&thread_root),
                kind: OutboxKind::Reply,
                payload_json: r#"{"text":"reply"}"#,
                last_error: None,
            },
        )
        .unwrap();
    assert_eq!(
        store.get_outbox_item(reply.id).unwrap(),
        Some(reply.clone())
    );
    assert_eq!(
        store
            .list_outbox_for_channel(&ws, &ch, Some(&thread_root))
            .unwrap()
            .len(),
        1
    );
    assert!(store.mark_outbox_sending(reply.id).unwrap());
    assert!(
        !store.mark_outbox_sending(reply.id).unwrap(),
        "claiming an already-sending row must return false"
    );
    assert!(store.delete_outbox_item(reply.id).unwrap());
    assert!(store.get_outbox_item(reply.id).unwrap().is_none());
}

#[test]
fn archive_indexing_transcripts_digest_and_outbox_edge_cases() {
    use slackless_store::NewHuddleTranscriptSegment;
    use slackless_types::{File, FileId, Profile, User};

    let mut store = Store::open_in_memory().unwrap();
    let ws = TeamId::from("T0ARCH");
    let ch = ChannelId::from("C0ARCH");
    let dm = ChannelId::from("D0ANON");

    // 1. Empty kinds slice returns empty immediately.
    assert!(
        store
            .list_conversations_by_kinds(&ws, &[])
            .unwrap()
            .is_empty()
    );

    // 2. Empty digest window returns empty.
    assert!(
        store
            .digest_since(&ws, 1_700_000_000, 10)
            .unwrap()
            .is_empty()
    );

    // 3. Upsert channel + DM with unknown peer user + messages (one live, one deleted, one with file).
    let conv = Conversation {
        id: ch.clone(),
        name: Some("archive-room".into()),
        is_channel: true,
        is_member: true,
        ..Default::default()
    };
    let dm_conv = Conversation {
        id: dm.clone(),
        is_im: true,
        user: Some(UserId::from("U0MISSING")),
        ..Default::default()
    };
    store.upsert_conversation(&ws, &conv).unwrap();
    store.upsert_conversation(&ws, &dm_conv).unwrap();

    let u1 = User {
        id: UserId::from("U0ALICE"),
        name: "alice".into(),
        profile: Profile {
            display_name: Some("Alice".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    store.upsert_user(&ws, &u1).unwrap();

    let f1 = File {
        id: FileId::from("F0AUDIO1"),
        name: Some("standup.m4a".into()),
        ..Default::default()
    };
    let m1 = Message {
        ts: Ts::from("1700000100.000100"),
        user: Some(UserId::from("U0ALICE")),
        text: "attached audio clip".into(),
        files: vec![f1],
        ..Default::default()
    };
    store.upsert_message(&ws, &ch, &m1).unwrap();

    let m_del = Message {
        ts: Ts::from("1700000105.000100"),
        user: Some(UserId::from("U0ALICE")),
        text: "deleted text".into(),
        ..Default::default()
    };
    store.upsert_message(&ws, &ch, &m_del).unwrap();
    store
        .record_deletion(&ws, &ch, &m_del.ts, None, None)
        .unwrap();

    let m_dm = Message {
        ts: Ts::from("1700000200.000100"),
        user: Some(UserId::from("U0MISSING")),
        text: "x".repeat(150),
        ..Default::default()
    };
    store.upsert_message(&ws, &dm, &m_dm).unwrap();

    // 4. `for_each_message_for_indexing` visits only non-deleted rows oldest-first and populates channel.
    let mut indexed = Vec::new();
    store
        .for_each_message_for_indexing(&ws, |cid, msg, fnames, _ftrans| {
            assert_eq!(msg.channel.as_ref(), Some(&cid));
            indexed.push((cid, msg.ts, fnames));
        })
        .unwrap();
    assert_eq!(indexed.len(), 2);
    assert_eq!(indexed[0].0, ch);
    assert!(indexed[0].2.contains("standup.m4a"));
    assert_eq!(indexed[1].0, dm);

    // 5. `list_file_transcripts_for_indexing` joins carrying message (and keeps orphans).
    store
        .upsert_file_transcript(
            &ws,
            &FileId::from("F0AUDIO1"),
            "whisper",
            "we shipped the release",
        )
        .unwrap();
    store
        .upsert_file_transcript(&ws, &FileId::from("F0ORPHAN"), "whisper", "standalone note")
        .unwrap();
    let fts = store.list_file_transcripts_for_indexing(&ws).unwrap();
    assert_eq!(fts.len(), 2);
    let attached = fts
        .iter()
        .find(|t| t.transcript.file_id.as_str() == "F0AUDIO1")
        .unwrap();
    assert_eq!(attached.channel.as_ref(), Some(&ch));
    assert_eq!(attached.message_ts.as_ref(), Some(&m1.ts));
    let orphan = fts
        .iter()
        .find(|t| t.transcript.file_id.as_str() == "F0ORPHAN")
        .unwrap();
    assert!(orphan.channel.is_none());

    // 6. `list_huddle_transcripts_for_indexing` & `huddle_transcript_segment` (hit + miss).
    let seg_id = store
        .insert_huddle_transcript_segment(
            &ws,
            &NewHuddleTranscriptSegment {
                channel: &ch,
                call_id: "R0CALL",
                thread_root_ts: Some(&m1.ts),
                speaker_id: Some(&UserId::from("U0ALICE")),
                lang: Some("en"),
                source: "chime",
                text: "deploy looks green",
                at_ms: 1_700_000_150_000,
            },
        )
        .unwrap();
    let all_segs = store.list_huddle_transcripts_for_indexing(&ws).unwrap();
    assert_eq!(all_segs.len(), 1);
    assert_eq!(all_segs[0].id, seg_id);
    assert_eq!(all_segs[0].text, "deploy looks green");
    let loaded = store
        .huddle_transcript_segment(&ws, seg_id)
        .unwrap()
        .expect("segment by id");
    assert_eq!(loaded, all_segs[0]);
    assert!(
        store
            .huddle_transcript_segment(&ws, 999_999)
            .unwrap()
            .is_none()
    );

    // 7. `digest_since` exercises unknown DM peer fallback to channel id + 120-char truncation.
    let digest = store.digest_since(&ws, 1_700_000_000, 10).unwrap();
    assert_eq!(digest.len(), 2);
    let dm_dig = digest.iter().find(|d| d.channel == dm).unwrap();
    assert_eq!(dm_dig.label, "D0ANON");
    assert_eq!(dm_dig.latest_author.as_deref(), Some("U0MISSING"));
    assert!(dm_dig.latest_text.ends_with('…'));
    assert_eq!(dm_dig.latest_text.chars().count(), 120);

    // 8. `OutboxKind` / `OutboxStatus` string round-trips, channel-root listing, and missing-ID ops.
    for kind in [
        OutboxKind::Message,
        OutboxKind::Reply,
        OutboxKind::ReactionAdd,
        OutboxKind::ReactionRemove,
    ] {
        assert_eq!(OutboxKind::parse(kind.as_str()), Some(kind));
    }
    assert_eq!(OutboxKind::parse("bogus"), None);
    for status in [
        OutboxStatus::Pending,
        OutboxStatus::Sending,
        OutboxStatus::Failed,
    ] {
        assert_eq!(OutboxStatus::parse(status.as_str()), Some(status));
    }
    assert_eq!(OutboxStatus::parse("bogus"), None);

    let root_item = store
        .enqueue_outbox(
            &ws,
            &NewOutboxItem {
                channel_id: &ch,
                thread_ts: None,
                kind: OutboxKind::ReactionAdd,
                payload_json: r#"{"ts":"1","name":"eyes"}"#,
                last_error: None,
            },
        )
        .unwrap();
    let root_list = store.list_outbox_for_channel(&ws, &ch, None).unwrap();
    assert_eq!(root_list.len(), 1);
    assert_eq!(root_list[0].id, root_item.id);
    assert!(!store.mark_outbox_sending(999_999).unwrap());
    assert!(!store.mark_outbox_failed(999_999, "err").unwrap());
    assert!(!store.reset_outbox_item_pending(999_999).unwrap());
    assert!(!store.delete_outbox_item(999_999).unwrap());
}
