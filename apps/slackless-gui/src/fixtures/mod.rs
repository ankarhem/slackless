//! The workspace snapshot views render: the bundled sample workspace for fixture mode (no Slack
//! credentials, no network), or the signed-in workspace read from the local store ([`live`]).
//! Both use the real `slackless_types` model, so views render the same either way.

mod model;
mod sample;

pub use model::{BotProfileMap, EmojiImage, Fixtures, PresenceMap, WorkspaceData};
pub use sample::{load, workspace};

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock, RwLock};

use serde_json::{Map, Value};
use slackless_core::api::ConversationCounts;
use slackless_core::auth::Session;
use slackless_core::store::Store;
use slackless_core::types::{ChannelId, Conversation, ConversationKind, Team};

/// Slack's one-click reactions from the user's prefs: none when turned off, the three
/// customized emoji when set, else Slack's defaults.
fn quick_reactions(prefs: &Value) -> Vec<String> {
    // `show_quick_reactions` decides, as the official client shows it on 2026-11-20: with it
    // off (and `user_customized_quick_reactions_display_feature` 2) Slack's hover bar has no
    // one-click reactions, in channels and DMs alike. Absent, they show.
    let shown = prefs.get("show_quick_reactions").and_then(|v| v.as_bool()) != Some(false);
    if !shown {
        return Vec::new();
    }
    let customized = prefs
        .get("user_customized_quick_reactions_has_customized")
        .and_then(|v| v.as_bool())
        == Some(true);
    let custom: Vec<String> = (1..=3)
        .filter_map(|i| {
            prefs
                .get(format!("user_customized_quick_reactions_emoji_{i}").as_str())
                .and_then(|v| v.as_str())
                .map(|name| name.trim_matches(':').to_owned())
        })
        .collect();
    if customized && !custom.is_empty() {
        custom
    } else {
        crate::actions::DEFAULT_QUICK_REACTIONS
            .map(String::from)
            .to_vec()
    }
}

/// Workspace-configured status suggestions from `team.prefs.custom_status_default_suggestions`
/// or `prefs.custom_status_presets` (`F-SHELL-015`). Returns `(emoji_shortcode, text, clear_after_ix)`.
pub fn status_presets_from_prefs(
    team_extra: &Map<String, Value>,
    user_prefs: &Value,
) -> Vec<(String, String, usize)> {
    fn duration_to_clear_ix(secs: Option<i64>) -> usize {
        match secs.unwrap_or(0) {
            s if s <= 0 => 0,
            s if s <= 1_800 => 1,
            s if s <= 3_600 => 2,
            s if s <= 14_400 => 3,
            s if s <= 86_400 => 4,
            _ => 5,
        }
    }
    fn parse_list(val: &Value) -> Vec<(String, String, usize)> {
        let Some(items) = val.as_array() else {
            return Vec::new();
        };
        items
            .iter()
            .filter_map(|entry| {
                if let Some(arr) = entry.as_array() {
                    let emoji = arr.first()?.as_str()?.trim().trim_matches(':');
                    let text = arr.get(1)?.as_str()?.trim();
                    if text.is_empty() {
                        return None;
                    }
                    let secs = arr.get(2).and_then(Value::as_i64);
                    Some((
                        if emoji.is_empty() {
                            "speech_balloon".to_owned()
                        } else {
                            emoji.to_owned()
                        },
                        text.to_owned(),
                        duration_to_clear_ix(secs),
                    ))
                } else if let Some(obj) = entry.as_object() {
                    let emoji = obj
                        .get("emoji")
                        .or_else(|| obj.get("status_emoji"))
                        .and_then(Value::as_str)
                        .unwrap_or("speech_balloon")
                        .trim()
                        .trim_matches(':');
                    let text = obj
                        .get("text")
                        .or_else(|| obj.get("status_text"))
                        .and_then(Value::as_str)?
                        .trim();
                    if text.is_empty() {
                        return None;
                    }
                    let secs = obj
                        .get("duration")
                        .or_else(|| obj.get("default_duration"))
                        .and_then(Value::as_i64);
                    Some((
                        if emoji.is_empty() {
                            "speech_balloon".to_owned()
                        } else {
                            emoji.to_owned()
                        },
                        text.to_owned(),
                        duration_to_clear_ix(secs),
                    ))
                } else {
                    None
                }
            })
            .collect()
    }
    for candidate in [
        team_extra
            .get("prefs")
            .and_then(|p| p.get("custom_status_default_suggestions")),
        user_prefs.get("custom_status_default_suggestions"),
        user_prefs.get("custom_status_presets"),
    ]
    .into_iter()
    .flatten()
    {
        let parsed = parse_list(candidate);
        if !parsed.is_empty() {
            return parsed;
        }
    }
    Vec::new()
}

/// The signed-in workspace as the local store has it (live mode): its conversations and people.
/// History is not part of the snapshot (the message list loads it per conversation,
/// [`crate::bridge::CoreBridge::history`]); only each DM's latest message rides along, one
/// batched store query feeding the DM list.
pub fn live(
    session: &Session,
    store: &Store,
    counts: &HashMap<ChannelId, ConversationCounts>,
) -> Fixtures {
    let team = &session.team_id;
    let conversations = store.conversations(team).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "could not read conversations from the local store");
        Vec::new()
    });
    let users = store.users(team).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "could not read people from the local store");
        Vec::new()
    });
    // Archived conversations never list; archived channels stay nameable.
    let (archived, conversations): (Vec<_>, Vec<_>) =
        conversations.into_iter().partition(|c| c.is_archived);
    let archived: Vec<Conversation> = archived
        .into_iter()
        .filter(|c| !matches!(c.kind(), ConversationKind::Im | ConversationKind::Mpim))
        .collect();
    let (dms, rooms): (Vec<_>, Vec<_>) = conversations
        .into_iter()
        .map(|mut c| {
            // `client.userBoot` omits read cursors; `client.counts` has them.
            if let Some(read) = counts.get(&c.id).and_then(|n| n.last_read.clone()) {
                c.last_read = Some(read);
            }
            c
        })
        .partition(|c| matches!(c.kind(), ConversationKind::Im | ConversationKind::Mpim));
    // Sidebar membership: `client.counts` lists exactly the sidebar's conversations hosted in
    // this workspace; until counts arrive, every stored channel is listed. Membership
    // (`users.conversations`) also covers channels joined since, but over-reports Slack Connect
    // channels the sidebar leaves out, so a shared channel needs counts too - unless another
    // organization hosts it, as it then never appears in this workspace's counts.
    let hosted_elsewhere = |c: &Conversation| {
        c.extra
            .get("conversation_host_id")
            .and_then(|host| host.as_str())
            .is_some_and(|host| host != team.as_str())
    };
    let (channels, directory) = rooms.into_iter().partition(|c| {
        counts.is_empty()
            || counts.contains_key(&c.id)
            || (c.is_member && (!c.is_ext_shared || hosted_elsewhere(c)))
    });
    let dms: Vec<Conversation> = dms
        .into_iter()
        .filter(|c| c.kind() == ConversationKind::Mpim || c.is_open)
        .collect();
    // Each DM's latest top-level message in one query: the DM list's previews, stamps and
    // order without loading any history.
    let latest = store
        .latest_messages(
            team,
            &dms.iter().map(|dm| dm.id.clone()).collect::<Vec<_>>(),
        )
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "could not read latest DM messages from the local store");
            HashMap::new()
        })
        .into_iter()
        // Like Slack, a deleted message never previews: the store skips it for the one before.
        .map(|(id, stored)| (id, stored.message))
        .collect();
    let domain = session
        .workspace_url
        .trim_start_matches("https://")
        .trim_end_matches('/')
        .trim_end_matches(".slack.com")
        .to_owned();
    let workspace = store
        .team(team)
        .ok()
        .flatten()
        .filter(|stored| !stored.name.is_empty())
        .unwrap_or_else(|| Team {
            id: team.clone(),
            name: domain.clone(),
            domain,
            ..Default::default()
        });
    // The own team first, then the connected organizations Slack Connect channels resolved
    // (`team.info` records stored beside it). A read failure keeps the own record alone.
    let mut teams = store.teams(team).unwrap_or_default();
    if !teams.iter().any(|stored| stored.id == *team) {
        teams.insert(0, workspace.clone());
    }
    let sections = store.sections(team).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "could not read sidebar sections from the local store");
        Vec::new()
    });
    let custom_emoji: BTreeMap<String, EmojiImage> = store
        .custom_emoji(team)
        .unwrap_or_default()
        .into_iter()
        .map(|(name, url)| (name, EmojiImage::Url(url.into())))
        .collect();
    let prefs = store.prefs(team).ok().flatten().unwrap_or_default();
    let frequent_emoji =
        crate::emoji::frequent_from_prefs(&prefs, |name| custom_emoji.contains_key(name));
    // Per-section sidebar state, kept as JSON text in the `channel_sections` pref:
    // `"c": "1"` marks a collapsed section.
    let collapsed_sections = prefs
        .get("channel_sections")
        .and_then(Value::as_str)
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .and_then(|sections| {
            let sections = sections.as_object()?;
            Some(
                sections
                    .iter()
                    .filter(|(_, state)| state.get("c").and_then(Value::as_str) == Some("1"))
                    .map(|(id, _)| id.clone())
                    .collect(),
            )
        })
        .unwrap_or_default();
    // Slack's name settings: the user's override (1 on, -1 off), 0 follows the team's.
    let name_setting = |user_override: &str, team_key: &str| match prefs
        .get(user_override)
        .and_then(Value::as_i64)
        .unwrap_or(0)
    {
        1 => true,
        -1 => false,
        _ => workspace
            .extra
            .get("prefs")
            .and_then(|prefs| prefs.get(team_key))
            .and_then(|flag| flag.as_bool().or_else(|| flag.as_i64().map(|n| n != 0)))
            .unwrap_or(false),
    };
    let real_names = name_setting("display_real_names_override", "display_real_names");
    let real_names_in_mentions = name_setting(
        "mentions_real_names_override",
        "display_real_names_in_mentions",
    );
    let status_presets = status_presets_from_prefs(&workspace.extra, &prefs);
    let cached_bots = store.bot_profiles(team).unwrap_or_default();
    let file_transcripts = store.all_file_transcripts(team).unwrap_or_default();
    Fixtures::from_parts(
        WorkspaceData {
            workspaces: vec![workspace],
            teams,
            self_user: session.user_id.clone(),
            users,
            usergroups: Vec::new(),
            app_commands: Vec::new(),
            channels,
            dms,
            messages: HashMap::new(),
            latest,
            threads: HashMap::new(),
            directory,
            archived,
            presence: PresenceMap::default(),
            bots: Arc::new(RwLock::new(cached_bots)),
            notify_levels: slackless_core::notify_levels(&prefs),
            quick_reactions: quick_reactions(&prefs),
            frequent_emoji,
            skin_tone: crate::emoji::skin_tone_from_prefs(&prefs),
            thumbnails: HashMap::new(),
            file_transcripts,
            profile_extras: HashMap::new(),
            custom_emoji,
            block_images: HashMap::new(),
            sample: false,
            sections,
            real_names,
            real_names_in_mentions,
            collapsed_sections,
            palette: crate::theme::palette_from_prefs(&prefs),
            status_presets,
            user_index: OnceLock::new(),
            conv_index: OnceLock::new(),
            team_index: OnceLock::new(),
        },
        counts.clone(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use slackless_core::types::{Message, Ts, UserId};

    #[test]
    fn indexed_lookups_plain_text_and_o1_with_counts_work() {
        let fixtures = load();
        assert_eq!(
            fixtures
                .user(&UserId::from("U0ALICE"))
                .map(|u| u.name.as_str()),
            Some("alice")
        );
        assert!(
            fixtures
                .conversation(&ChannelId::from("C0GENERAL"))
                .is_some()
        );
        let sample_msg = Message {
            ts: Ts::from("1700000001.000100"),
            channel: Some(ChannelId::from("C0GENERAL")),
            user: Some(UserId::from("U0ALICE")),
            text: "cc <@U0BOB> and <@U0CARA|cara> :rocket:".into(),
            ..Default::default()
        };
        assert_eq!(fixtures.plain_text(&sample_msg), "cc @bob and @cara 🚀");
        let updated = fixtures.with_counts(HashMap::from([(
            ChannelId::from("C0GENERAL"),
            ConversationCounts {
                id: ChannelId::from("C0GENERAL"),
                last_read: Some(Ts::from("1700000000.000100")),
                has_unreads: true,
                mention_count: 2,
                latest: None,
                ..Default::default()
            },
        )]));
        // O(1) sharing: `with_counts` reuses the exact same `Arc<WorkspaceData>` & cached indices.
        assert!(Arc::ptr_eq(&fixtures.data, &updated.data));
        assert_eq!(
            updated
                .last_read(&ChannelId::from("C0GENERAL"))
                .map(Ts::as_str),
            Some("1700000000.000100")
        );
        assert_eq!(
            updated
                .counts
                .get(&ChannelId::from("C0GENERAL"))
                .map(|c| c.mention_count),
            Some(2)
        );
    }

    #[test]
    fn with_file_transcript_stores_trims_and_clears_empty() {
        let fixtures = load();
        let fid = slackless_core::types::FileId::from("F_CLIP_1");
        assert_eq!(fixtures.file_transcript(&fid), None);

        let with_tr =
            fixtures.with_file_transcript(fid.clone(), "  hello from the demo clip \n".into());
        assert_eq!(
            with_tr.file_transcript(&fid),
            Some("hello from the demo clip")
        );
        // Original snapshot remains immutable.
        assert_eq!(fixtures.file_transcript(&fid), None);

        // Blank transcript clears the entry cleanly.
        let cleared = with_tr.with_file_transcript(fid.clone(), "   ".into());
        assert_eq!(cleared.file_transcript(&fid), None);
    }
}
