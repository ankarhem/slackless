//! Local Catch-Up / Daily Briefing engine over the local SQLite archive(s).
//!
//! Aggregates across a single workspace or all signed-in/archived workspaces over a configurable
//! time window (`SinceLastVisit`, `Last24h`, `Last7d`, or custom cutoff):
//! - Direct mentions (`<@self>`, broadcasts `<!here>` / `<!channel>`, and incoming DMs)
//! - Unread channels with message counts and top excerpts
//! - Active threads with new replies in the window
//! - Newly transcribed audio/video clips and huddle transcripts

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::SharedStore;
use crate::client::read_store;
use crate::domain::mrkdwn::mrkdwn_to_plain;
use crate::export::user_display_map;
use crate::notify::{NotifyLevel, notify_levels};
use crate::search::truncate_chars;
use crate::store::{Result as StoreResult, Store, StoredMessage};
use crate::types::{
    ChannelId, Conversation, ConversationKind, FileId, MessageSubtype, TeamId, Ts, UserId,
};

/// `local_state` key tracking the unix-seconds timestamp of the user's last Catch-Up visit.
pub const CATCHUP_LAST_VISIT_KEY: &str = "catchup_last_visit_secs";

/// Maximum recent messages scanned per workspace in one Catch-Up build (index-backed).
const MESSAGE_SCAN_LIMIT: usize = 2_000;
/// Default cap per section when callers pass a standard briefing limit.
pub const DEFAULT_CATCHUP_LIMIT: usize = 25;
/// Top excerpts kept per unread channel card.
const EXCERPTS_PER_CHANNEL: usize = 3;

/// Configurable time window for a Catch-Up briefing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CatchUpWindow {
    /// Activity since the workspace's stored last Catch-Up visit (falls back to 24h if never set).
    SinceLastVisit,
    /// Last 24 hours.
    Last24h,
    /// Last 7 days.
    Last7d,
    /// Explicit unix-seconds cutoff (e.g. CLI `--since 12h` or `--since 2026-11-20`).
    Custom { since_secs: i64 },
}

impl CatchUpWindow {
    /// Resolve the unix-seconds cutoff for a workspace given `now_secs` and its optional stored
    /// `last_visit_secs`.
    pub fn cutoff_secs(self, now_secs: i64, last_visit_secs: Option<i64>) -> i64 {
        match self {
            Self::SinceLastVisit => last_visit_secs
                .filter(|&v| v > 0 && v <= now_secs)
                .unwrap_or_else(|| now_secs.saturating_sub(86_400)),
            Self::Last24h => now_secs.saturating_sub(86_400),
            Self::Last7d => now_secs.saturating_sub(7 * 86_400),
            Self::Custom { since_secs } => since_secs,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::SinceLastVisit => "Since last visit",
            Self::Last24h => "Last 24h",
            Self::Last7d => "Last 7d",
            Self::Custom { .. } => "Custom",
        }
    }
}

/// Classification of a mention item in a Catch-Up briefing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MentionKind {
    DirectMention,
    DirectMessage,
    Broadcast,
}

impl MentionKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::DirectMention => "Mention",
            Self::DirectMessage => "Direct message",
            Self::Broadcast => "Channel broadcast",
        }
    }
}

/// Classification of a newly transcribed audio/video/huddle item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptKind {
    FileClip,
    Huddle,
}

impl TranscriptKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::FileClip => "Clip",
            Self::Huddle => "Huddle",
        }
    }
}

/// One workspace covered by a [`CatchUpBriefing`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CatchUpWorkspaceSummary {
    pub team_id: TeamId,
    pub name: Option<String>,
    pub cutoff_secs: i64,
}

/// A direct mention, incoming DM, or channel broadcast inside the Catch-Up window.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CatchUpMention {
    pub workspace: TeamId,
    pub workspace_name: Option<String>,
    pub channel: ChannelId,
    pub channel_label: String,
    pub ts: Ts,
    pub thread_ts: Option<Ts>,
    pub author_id: Option<UserId>,
    pub author_name: String,
    pub excerpt: String,
    pub created_at: i64,
    pub kind: MentionKind,
}

/// One message excerpt shown inside an unread channel summary card.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CatchUpExcerpt {
    pub ts: Ts,
    pub author_name: String,
    pub text: String,
    pub created_at: i64,
}

/// One unread conversation with its unread message count and newest excerpts.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CatchUpUnreadChannel {
    pub workspace: TeamId,
    pub workspace_name: Option<String>,
    pub channel: ChannelId,
    pub channel_label: String,
    pub is_dm: bool,
    pub message_count: u64,
    pub mention_count: u64,
    pub latest_ts: Ts,
    pub latest_at: i64,
    pub excerpts: Vec<CatchUpExcerpt>,
}

/// One active thread that received new replies inside the Catch-Up window.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CatchUpThread {
    pub workspace: TeamId,
    pub workspace_name: Option<String>,
    pub channel: ChannelId,
    pub channel_label: String,
    pub thread_ts: Ts,
    pub root_author: String,
    pub root_excerpt: String,
    pub new_reply_count: u64,
    pub total_reply_count: u64,
    pub latest_reply_ts: Ts,
    pub latest_reply_author: String,
    pub latest_reply_excerpt: String,
    pub latest_reply_at: i64,
}

/// One newly transcribed audio/video clip or huddle session inside the Catch-Up window.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CatchUpTranscript {
    pub workspace: TeamId,
    pub workspace_name: Option<String>,
    pub kind: TranscriptKind,
    pub channel: Option<ChannelId>,
    pub channel_label: Option<String>,
    pub message_ts: Option<Ts>,
    pub file_id: Option<FileId>,
    pub call_id: Option<String>,
    pub title: String,
    pub source: String,
    pub excerpt: String,
    pub updated_at: i64,
}

/// Complete aggregated Local Catch-Up / Daily Briefing snapshot.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CatchUpBriefing {
    pub generated_at_secs: i64,
    pub window: CatchUpWindow,
    pub cutoff_secs: i64,
    pub workspaces: Vec<CatchUpWorkspaceSummary>,
    pub mentions: Vec<CatchUpMention>,
    pub unread_channels: Vec<CatchUpUnreadChannel>,
    pub active_threads: Vec<CatchUpThread>,
    pub transcripts: Vec<CatchUpTranscript>,
}

impl CatchUpBriefing {
    pub fn empty(window: CatchUpWindow, now_secs: i64) -> Self {
        Self {
            generated_at_secs: now_secs,
            window,
            cutoff_secs: window.cutoff_secs(now_secs, None),
            workspaces: Vec::new(),
            mentions: Vec::new(),
            unread_channels: Vec::new(),
            active_threads: Vec::new(),
            transcripts: Vec::new(),
        }
    }

    pub fn total_items(&self) -> usize {
        self.mentions.len()
            + self.unread_channels.len()
            + self.active_threads.len()
            + self.transcripts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.total_items() == 0
    }
}

/// Mounted workspace handle for building a briefing across open in-memory / live `SharedStore`s.
#[derive(Clone)]
pub struct WorkspaceCatchUpTarget {
    pub workspace: TeamId,
    pub workspace_name: Option<String>,
    pub self_user: Option<UserId>,
    pub store: SharedStore,
}

/// Persist the user's latest Catch-Up visit timestamp for `workspace` in `local_state`.
pub fn record_catchup_visit(
    store: &mut Store,
    workspace: &TeamId,
    now_secs: i64,
) -> StoreResult<()> {
    store.set_local(workspace, CATCHUP_LAST_VISIT_KEY, &now_secs.to_string())
}

/// Read the stored last Catch-Up visit timestamp for `workspace`, if recorded.
pub fn read_catchup_last_visit(store: &Store, workspace: &TeamId) -> StoreResult<Option<i64>> {
    Ok(store
        .local(workspace, CATCHUP_LAST_VISIT_KEY)?
        .and_then(|raw| raw.trim().parse::<i64>().ok()))
}

/// Build a Catch-Up briefing for a single [`Store`] workspace.
pub fn build_workspace_catchup(
    store: &Store,
    workspace: &TeamId,
    fallback_name: Option<&str>,
    self_user: Option<&UserId>,
    window: CatchUpWindow,
    now_secs: i64,
    limit: usize,
) -> StoreResult<CatchUpBriefing> {
    let mut briefing = CatchUpBriefing::empty(window, now_secs);
    if limit == 0 {
        return Ok(briefing);
    }
    let last_visit = read_catchup_last_visit(store, workspace)?;
    let cutoff = window.cutoff_secs(now_secs, last_visit);
    briefing.cutoff_secs = cutoff;

    collect_workspace_into(
        store,
        workspace,
        fallback_name,
        self_user,
        cutoff,
        limit,
        &mut briefing,
    )?;
    sort_and_cap_briefing(&mut briefing, limit);
    Ok(briefing)
}

/// Aggregate a Catch-Up briefing across one or more active/parked [`WorkspaceCatchUpTarget`]s.
pub fn build_catchup_across_stores(
    targets: &[WorkspaceCatchUpTarget],
    window: CatchUpWindow,
    now_secs: i64,
    limit: usize,
) -> StoreResult<CatchUpBriefing> {
    let mut briefing = CatchUpBriefing::empty(window, now_secs);
    if limit == 0 || targets.is_empty() {
        return Ok(briefing);
    }
    let mut seen_ws: HashSet<TeamId> = HashSet::new();
    let mut min_cutoff: Option<i64> = None;

    for target in targets {
        if !seen_ws.insert(target.workspace.clone()) {
            continue;
        }
        let store = read_store(&target.store);
        let last_visit = read_catchup_last_visit(&store, &target.workspace)?;
        let cutoff = window.cutoff_secs(now_secs, last_visit);
        min_cutoff = Some(min_cutoff.map_or(cutoff, |prev| prev.min(cutoff)));
        collect_workspace_into(
            &store,
            &target.workspace,
            target.workspace_name.as_deref(),
            target.self_user.as_ref(),
            cutoff,
            limit,
            &mut briefing,
        )?;
    }

    if let Some(cutoff) = min_cutoff {
        briefing.cutoff_secs = cutoff;
    }
    sort_and_cap_briefing(&mut briefing, limit);
    Ok(briefing)
}

/// Aggregate a Catch-Up briefing across on-disk `<team_id>.sqlite` archive files.
pub fn build_catchup_disk_archives(
    archives: &[(TeamId, PathBuf)],
    self_users: &HashMap<TeamId, UserId>,
    window: CatchUpWindow,
    now_secs: i64,
    limit: usize,
) -> StoreResult<CatchUpBriefing> {
    let mut briefing = CatchUpBriefing::empty(window, now_secs);
    if limit == 0 || archives.is_empty() {
        return Ok(briefing);
    }
    let mut seen_ws: HashSet<TeamId> = HashSet::new();
    let mut min_cutoff: Option<i64> = None;

    for (ws, path) in archives {
        if !path.is_file() {
            continue;
        }
        let store = Store::open_for_reading(path)?;
        let mut inner_workspaces = store.list_workspaces()?;
        if inner_workspaces.is_empty() {
            inner_workspaces.push(ws.clone());
        }
        for inner_ws in inner_workspaces {
            if !seen_ws.insert(inner_ws.clone()) {
                continue;
            }
            let last_visit = read_catchup_last_visit(&store, &inner_ws)?;
            let cutoff = window.cutoff_secs(now_secs, last_visit);
            min_cutoff = Some(min_cutoff.map_or(cutoff, |prev| prev.min(cutoff)));
            collect_workspace_into(
                &store,
                &inner_ws,
                None,
                self_users.get(&inner_ws),
                cutoff,
                limit,
                &mut briefing,
            )?;
        }
    }

    if let Some(cutoff) = min_cutoff {
        briefing.cutoff_secs = cutoff;
    }
    sort_and_cap_briefing(&mut briefing, limit);
    Ok(briefing)
}

fn collect_workspace_into(
    store: &Store,
    workspace: &TeamId,
    fallback_name: Option<&str>,
    self_user: Option<&UserId>,
    cutoff_secs: i64,
    limit: usize,
    out: &mut CatchUpBriefing,
) -> StoreResult<()> {
    let workspace_name = store
        .team(workspace)?
        .map(|t| t.name)
        .filter(|n| !n.trim().is_empty())
        .or_else(|| {
            fallback_name
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(ToOwned::to_owned)
        });
    out.workspaces.push(CatchUpWorkspaceSummary {
        team_id: workspace.clone(),
        name: workspace_name.clone(),
        cutoff_secs,
    });

    let conversations: HashMap<ChannelId, Conversation> = store
        .conversations(workspace)?
        .into_iter()
        .map(|c| (c.id.clone(), c))
        .collect();
    let users = user_display_map(store, workspace)?;
    let levels = store
        .prefs(workspace)?
        .map(|p| notify_levels(&p))
        .unwrap_or_default();

    let messages = store.messages_since(workspace, cutoff_secs, MESSAGE_SCAN_LIMIT)?;

    // Track per-channel unread aggregations and active thread replies in one pass.
    struct ChannelAgg {
        is_dm: bool,
        count: u64,
        mentions: u64,
        latest_ts: Ts,
        latest_at: i64,
        excerpts: Vec<CatchUpExcerpt>,
    }
    struct ThreadAgg {
        new_reply_count: u64,
        latest_reply: StoredMessage,
    }

    let mut channel_aggs: HashMap<ChannelId, ChannelAgg> = HashMap::new();
    let mut thread_aggs: HashMap<(ChannelId, Ts), ThreadAgg> = HashMap::new();

    for stored in &messages {
        let msg = &stored.message;
        if msg.hidden || msg.is_ephemeral {
            continue;
        }
        let from_self = self_user.is_some_and(|me| msg.user.as_ref() == Some(me));
        let conv = conversations.get(&stored.channel);
        let is_dm =
            conv.is_some_and(|c| matches!(c.kind(), ConversationKind::Im | ConversationKind::Mpim));
        let is_muted = levels.get(&stored.channel) == Some(&NotifyLevel::Muted);
        let is_reply = msg.thread_ts.as_ref().is_some_and(|tts| *tts != msg.ts);
        let is_broadcast = msg.subtype == Some(MessageSubtype::ThreadBroadcast);
        let channel_label = format_channel_label(&stored.channel, conv, &users);
        let author_name = format_author(stored, &users);
        let created_at = msg.ts.secs().unwrap_or(0);

        // 1. Direct mentions / DMs / broadcasts (excluding own posts).
        if !from_self
            && let Some(kind) = detect_mention_kind(&msg.text, self_user, is_dm)
            && (!is_muted || kind == MentionKind::DirectMention)
        {
            out.mentions.push(CatchUpMention {
                workspace: workspace.clone(),
                workspace_name: workspace_name.clone(),
                channel: stored.channel.clone(),
                channel_label: channel_label.clone(),
                ts: msg.ts.clone(),
                thread_ts: is_reply.then(|| msg.thread_ts.clone()).flatten(),
                author_id: msg.user.clone(),
                author_name: author_name.clone(),
                excerpt: clean_excerpt(&msg.text, 160),
                created_at,
                kind,
            });
        }

        // 2. Unread channel-level messages (roots, plain messages, broadcasts; excluding own posts
        //    and messages at or behind the stored `last_read` cursor).
        let last_read = conv.and_then(|c| c.last_read.as_ref());
        let is_unread_msg = !from_self
            && (!is_reply || is_broadcast)
            && last_read.is_none_or(|read| msg.ts > *read);
        if is_unread_msg {
            let direct_hit = detect_mention_kind(&msg.text, self_user, is_dm).is_some();
            if !is_muted || direct_hit {
                let agg = channel_aggs
                    .entry(stored.channel.clone())
                    .or_insert_with(|| ChannelAgg {
                        is_dm,
                        count: 0,
                        mentions: 0,
                        latest_ts: msg.ts.clone(),
                        latest_at: created_at,
                        excerpts: Vec::with_capacity(EXCERPTS_PER_CHANNEL),
                    });
                agg.count = agg.count.saturating_add(1);
                if direct_hit {
                    agg.mentions = agg.mentions.saturating_add(1);
                }
                if msg.ts > agg.latest_ts {
                    agg.latest_ts = msg.ts.clone();
                    agg.latest_at = created_at;
                }
                if agg.excerpts.len() < EXCERPTS_PER_CHANNEL {
                    let excerpt_text = clean_excerpt(&msg.text, 120);
                    if !excerpt_text.is_empty() {
                        agg.excerpts.push(CatchUpExcerpt {
                            ts: msg.ts.clone(),
                            author_name: author_name.clone(),
                            text: excerpt_text,
                            created_at,
                        });
                    }
                }
            }
        }

        // 3. Active threads with new replies inside the window.
        if is_reply && let Some(root_ts) = msg.thread_ts.clone() {
            thread_aggs
                .entry((stored.channel.clone(), root_ts))
                .and_modify(|agg| {
                    agg.new_reply_count = agg.new_reply_count.saturating_add(1);
                    if msg.ts > agg.latest_reply.message.ts {
                        agg.latest_reply = stored.clone();
                    }
                })
                .or_insert_with(|| ThreadAgg {
                    new_reply_count: 1,
                    latest_reply: stored.clone(),
                });
        }
    }

    for (channel, agg) in channel_aggs {
        let conv = conversations.get(&channel);
        let channel_label = format_channel_label(&channel, conv, &users);
        out.unread_channels.push(CatchUpUnreadChannel {
            workspace: workspace.clone(),
            workspace_name: workspace_name.clone(),
            channel,
            channel_label,
            is_dm: agg.is_dm,
            message_count: agg.count,
            mention_count: agg.mentions,
            latest_ts: agg.latest_ts,
            latest_at: agg.latest_at,
            excerpts: agg.excerpts,
        });
    }

    // Batch-fetch thread roots for active threads in one read snapshot.
    if !thread_aggs.is_empty() {
        let mut thread_entries: Vec<((ChannelId, Ts), ThreadAgg)> =
            thread_aggs.into_iter().collect();
        thread_entries.sort_by(|a, b| {
            b.1.latest_reply
                .message
                .ts
                .cmp(&a.1.latest_reply.message.ts)
                .then_with(|| b.1.new_reply_count.cmp(&a.1.new_reply_count))
        });
        thread_entries.truncate(limit);
        let root_keys: Vec<(ChannelId, Ts)> =
            thread_entries.iter().map(|(k, _)| k.clone()).collect();
        let roots = store.messages_by_keys(workspace, &root_keys)?;
        for ((channel, thread_ts), agg) in thread_entries {
            let conv = conversations.get(&channel);
            let channel_label = format_channel_label(&channel, conv, &users);
            let root = roots.get(&(channel.clone(), thread_ts.clone()));
            let root_author = root
                .map(|r| format_author(r, &users))
                .unwrap_or_else(|| "Unknown".to_owned());
            let root_excerpt = root
                .map(|r| clean_excerpt(&r.message.text, 120))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "Thread".to_owned());
            let total_reply_count = root
                .and_then(|r| r.message.reply_count.map(u64::from))
                .unwrap_or(agg.new_reply_count)
                .max(agg.new_reply_count);
            let latest_reply_author = format_author(&agg.latest_reply, &users);
            let latest_reply_excerpt = clean_excerpt(&agg.latest_reply.message.text, 140);
            let latest_reply_at = agg.latest_reply.message.ts.secs().unwrap_or(0);
            out.active_threads.push(CatchUpThread {
                workspace: workspace.clone(),
                workspace_name: workspace_name.clone(),
                channel,
                channel_label,
                thread_ts,
                root_author,
                root_excerpt,
                new_reply_count: agg.new_reply_count,
                total_reply_count,
                latest_reply_ts: agg.latest_reply.message.ts,
                latest_reply_author,
                latest_reply_excerpt,
                latest_reply_at,
            });
        }
    }

    // 4a. Newly transcribed audio/video clips (`file_transcript`).
    let clips = store.recent_file_transcripts_since(workspace, cutoff_secs, limit)?;
    for clip in clips {
        let channel_label = clip
            .channel
            .as_ref()
            .map(|ch| format_channel_label(ch, conversations.get(ch), &users));
        let title = clip
            .title
            .unwrap_or_else(|| format!("Clip {}", clip.file_id.as_str()));
        out.transcripts.push(CatchUpTranscript {
            workspace: workspace.clone(),
            workspace_name: workspace_name.clone(),
            kind: TranscriptKind::FileClip,
            channel: clip.channel,
            channel_label,
            message_ts: clip.message_ts,
            file_id: Some(clip.file_id),
            call_id: None,
            title,
            source: clip.source,
            excerpt: clean_excerpt(&clip.text, 160),
            updated_at: clip.updated_at,
        });
    }

    // 4b. Newly transcribed huddle sessions (`huddle_transcript`).
    let huddles = store.recent_huddle_sessions_since(workspace, cutoff_secs, limit)?;
    for session in huddles {
        let channel_label = format_channel_label(
            &session.channel,
            conversations.get(&session.channel),
            &users,
        );
        let title = format!("Huddle in {channel_label}");
        let source = if session.languages.is_empty() {
            "huddle".to_owned()
        } else {
            format!("huddle ({})", session.languages.join(", "))
        };
        out.transcripts.push(CatchUpTranscript {
            workspace: workspace.clone(),
            workspace_name: workspace_name.clone(),
            kind: TranscriptKind::Huddle,
            channel: Some(session.channel),
            channel_label: Some(channel_label),
            message_ts: session.thread_root_ts,
            file_id: None,
            call_id: Some(session.call_id),
            title,
            source,
            excerpt: clean_excerpt(&session.preview, 160),
            updated_at: session.ended_at_ms / 1_000,
        });
    }

    Ok(())
}

fn sort_and_cap_briefing(briefing: &mut CatchUpBriefing, limit: usize) {
    briefing.mentions.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.ts.cmp(&a.ts))
    });
    briefing.mentions.truncate(limit);

    briefing.unread_channels.sort_by(|a, b| {
        b.mention_count
            .cmp(&a.mention_count)
            .then_with(|| b.message_count.cmp(&a.message_count))
            .then_with(|| b.latest_at.cmp(&a.latest_at))
    });
    briefing.unread_channels.truncate(limit);

    briefing.active_threads.sort_by(|a, b| {
        b.latest_reply_at
            .cmp(&a.latest_reply_at)
            .then_with(|| b.new_reply_count.cmp(&a.new_reply_count))
    });
    briefing.active_threads.truncate(limit);

    briefing
        .transcripts
        .sort_by_key(|a| std::cmp::Reverse(a.updated_at));
    briefing.transcripts.truncate(limit);
}

fn detect_mention_kind(text: &str, self_user: Option<&UserId>, is_dm: bool) -> Option<MentionKind> {
    if let Some(me) = self_user {
        let bare = format!("<@{}>", me.as_str());
        let with_alias = format!("<@{}|", me.as_str());
        if text.contains(&bare) || text.contains(&with_alias) {
            return Some(MentionKind::DirectMention);
        }
    }
    if is_dm {
        return Some(MentionKind::DirectMessage);
    }
    if text.contains("<!here>")
        || text.contains("<!here|")
        || text.contains("<!channel>")
        || text.contains("<!channel|")
        || text.contains("<!everyone>")
        || text.contains("<!everyone|")
    {
        return Some(MentionKind::Broadcast);
    }
    None
}

fn format_channel_label(
    channel: &ChannelId,
    conv: Option<&Conversation>,
    users: &HashMap<UserId, String>,
) -> String {
    match conv {
        Some(c) if c.kind() == ConversationKind::Im => c
            .user
            .as_ref()
            .and_then(|u| users.get(u))
            .map(|name| format!("@{name}"))
            .or_else(|| c.name.as_ref().map(|n| format!("@{n}")))
            .unwrap_or_else(|| channel.to_string()),
        Some(c) => c
            .name
            .as_ref()
            .map(|n| format!("#{n}"))
            .unwrap_or_else(|| channel.to_string()),
        None => channel.to_string(),
    }
}

fn format_author(stored: &StoredMessage, users: &HashMap<UserId, String>) -> String {
    stored
        .message
        .user
        .as_ref()
        .and_then(|u| users.get(u).cloned())
        .or_else(|| stored.message.username.clone())
        .or_else(|| {
            stored
                .message
                .bot_profile
                .as_ref()
                .map(|b| b.name.clone())
                .filter(|n| !n.is_empty())
        })
        .or_else(|| stored.message.user.as_ref().map(|u| u.as_str().to_owned()))
        .unwrap_or_else(|| "Unknown".to_owned())
}

fn clean_excerpt(raw: &str, max_chars: usize) -> String {
    truncate_chars(&mrkdwn_to_plain(raw), max_chars)
}

/// Format a [`CatchUpBriefing`] into concise, scannable plain text for the CLI.
pub fn format_catchup_human(briefing: &CatchUpBriefing, multi_workspace: bool) -> String {
    if briefing.is_empty() {
        return format!(
            "All caught up ({}) — no mentions, unread channels, active threads, or new transcripts.\n",
            briefing.window.label()
        );
    }
    let mut out = String::new();
    let ws_tag = |ws: &TeamId, name: Option<&str>| -> String {
        if !multi_workspace {
            String::new()
        } else {
            format!("[{}] ", name.unwrap_or(ws.as_str()))
        }
    };

    if !briefing.mentions.is_empty() {
        out.push_str(&format!("MENTIONS ({})\n", briefing.mentions.len()));
        for m in &briefing.mentions {
            let tag = ws_tag(&m.workspace, m.workspace_name.as_deref());
            out.push_str(&format!(
                "  {tag}{}\t{}\t{}: {}\n",
                m.channel_label, m.ts, m.author_name, m.excerpt
            ));
        }
    }

    if !briefing.unread_channels.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!(
            "UNREAD CHANNELS ({})\n",
            briefing.unread_channels.len()
        ));
        for ch in &briefing.unread_channels {
            let tag = ws_tag(&ch.workspace, ch.workspace_name.as_deref());
            let mention_note = if ch.mention_count > 0 {
                format!(", {} mentions", ch.mention_count)
            } else {
                String::new()
            };
            let top = ch
                .excerpts
                .first()
                .map(|e| format!("{}: {}", e.author_name, e.text))
                .unwrap_or_default();
            out.push_str(&format!(
                "  {tag}{}\t{} msg{mention_note}\t{top}\n",
                ch.channel_label, ch.message_count
            ));
        }
    }

    if !briefing.active_threads.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!(
            "ACTIVE THREADS ({})\n",
            briefing.active_threads.len()
        ));
        for t in &briefing.active_threads {
            let tag = ws_tag(&t.workspace, t.workspace_name.as_deref());
            out.push_str(&format!(
                "  {tag}{}\t{}\t+{} replies\t{}: {}  -> {}: {}\n",
                t.channel_label,
                t.thread_ts,
                t.new_reply_count,
                t.root_author,
                t.root_excerpt,
                t.latest_reply_author,
                t.latest_reply_excerpt
            ));
        }
    }

    if !briefing.transcripts.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("TRANSCRIPTS ({})\n", briefing.transcripts.len()));
        for tr in &briefing.transcripts {
            let tag = ws_tag(&tr.workspace, tr.workspace_name.as_deref());
            out.push_str(&format!(
                "  {tag}[{}] {}\t{}\t{}\n",
                tr.kind.label(),
                tr.title,
                tr.source,
                tr.excerpt
            ));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::store::NewHuddleTranscriptSegment;
    use crate::types::{File, Message, Profile, Team, User};

    fn seed_test_workspace(store: &mut Store, team_id: &str, team_name: &str, now: i64) {
        let ws = TeamId::from(team_id);
        store
            .upsert_team(
                &ws,
                &Team {
                    id: ws.clone(),
                    name: team_name.into(),
                    domain: team_name.to_lowercase(),
                    ..Default::default()
                },
            )
            .unwrap();
        store
            .upsert_users(
                &ws,
                &[
                    User {
                        id: UserId::from("U_ME"),
                        name: "me".into(),
                        profile: Profile {
                            display_name: Some("Me".into()),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    User {
                        id: UserId::from("U_ALICE"),
                        name: "alice".into(),
                        profile: Profile {
                            display_name: Some("Alice".into()),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                ],
            )
            .unwrap();
        store
            .upsert_conversations(
                &ws,
                &[
                    Conversation {
                        id: ChannelId::from("C_ENG"),
                        name: Some("eng".into()),
                        is_channel: true,
                        is_member: true,
                        last_read: Some(Ts::from(format!("{}.000000", now - 3600))),
                        ..Default::default()
                    },
                    Conversation {
                        id: ChannelId::from("C_MUTED"),
                        name: Some("noisy".into()),
                        is_channel: true,
                        is_member: true,
                        ..Default::default()
                    },
                    Conversation {
                        id: ChannelId::from("D_ALICE"),
                        is_im: true,
                        user: Some(UserId::from("U_ALICE")),
                        ..Default::default()
                    },
                ],
            )
            .unwrap();
        let prefs = serde_json::json!({
            "all_notifications_prefs": serde_json::json!({
                "channels": {
                    "C_MUTED": { "muted": true }
                }
            }).to_string()
        });
        store.replace_prefs(&ws, &prefs).unwrap();

        // Already-read message in C_ENG (before last_read) + root of active thread.
        let thread_root_ts = Ts::from(format!("{}.000100", now - 7200));
        let mut root = Message {
            ts: thread_root_ts.clone(),
            user: Some(UserId::from("U_ME")),
            text: "Release checklist thread".into(),
            reply_count: Some(2),
            ..Default::default()
        };
        root.files.push(File {
            id: FileId::from("F_DEMO"),
            name: Some("walkthrough.mp4".into()),
            ..Default::default()
        });

        let unread_mention = Message {
            ts: Ts::from(format!("{}.000100", now - 600)),
            user: Some(UserId::from("U_ALICE")),
            text: "Hey <@U_ME> please review the deploy plan".into(),
            ..Default::default()
        };
        let thread_reply = Message {
            ts: Ts::from(format!("{}.000100", now - 300)),
            thread_ts: Some(thread_root_ts),
            user: Some(UserId::from("U_ALICE")),
            text: "Tests are green now!".into(),
            ..Default::default()
        };
        let own_msg = Message {
            ts: Ts::from(format!("{}.000100", now - 200)),
            user: Some(UserId::from("U_ME")),
            text: "Thanks!".into(),
            ..Default::default()
        };
        store
            .upsert_messages(
                &ws,
                &ChannelId::from("C_ENG"),
                &[root, unread_mention, thread_reply, own_msg],
            )
            .unwrap();

        // Muted channel noise (should NOT appear as unread) + direct mention (SHOULD appear).
        store
            .upsert_message(
                &ws,
                &ChannelId::from("C_MUTED"),
                &Message {
                    ts: Ts::from(format!("{}.000100", now - 500)),
                    user: Some(UserId::from("U_ALICE")),
                    text: "Just chatter in muted".into(),
                    ..Default::default()
                },
            )
            .unwrap();

        // Direct message from Alice.
        store
            .upsert_message(
                &ws,
                &ChannelId::from("D_ALICE"),
                &Message {
                    ts: Ts::from(format!("{}.000100", now - 400)),
                    user: Some(UserId::from("U_ALICE")),
                    text: "Got a minute to sync?".into(),
                    ..Default::default()
                },
            )
            .unwrap();

        // Newly transcribed file clip and huddle.
        store
            .upsert_file_transcript(
                &ws,
                &FileId::from("F_DEMO"),
                "parakeet",
                "Walkthrough of the offline outbox architecture",
            )
            .unwrap();
        store
            .insert_huddle_transcript_segment(
                &ws,
                &NewHuddleTranscriptSegment {
                    channel: &ChannelId::from("C_ENG"),
                    call_id: "CALL_1",
                    thread_root_ts: None,
                    speaker_id: Some(&UserId::from("U_ALICE")),
                    lang: Some("en"),
                    source: "chime",
                    text: "Let us ship the daily briefing today",
                    at_ms: (now - 120) * 1_000,
                },
            )
            .unwrap();
    }

    #[test]
    fn catchup_aggregates_mentions_unreads_threads_and_transcripts() {
        let now = crate::types::time::now_epoch_secs();
        let mut store = Store::open_in_memory().unwrap();
        seed_test_workspace(&mut store, "T1", "Acme", now);

        let briefing = build_workspace_catchup(
            &store,
            &TeamId::from("T1"),
            None,
            Some(&UserId::from("U_ME")),
            CatchUpWindow::Last24h,
            now,
            DEFAULT_CATCHUP_LIMIT,
        )
        .unwrap();

        assert_eq!(briefing.workspaces.len(), 1);
        assert_eq!(briefing.workspaces[0].name.as_deref(), Some("Acme"));

        // Mentions: DM from Alice + <@U_ME> in #eng (self post excluded, muted chatter excluded).
        assert_eq!(briefing.mentions.len(), 2);
        assert_eq!(briefing.mentions[0].kind, MentionKind::DirectMessage);
        assert_eq!(briefing.mentions[0].channel_label, "@Alice");
        assert_eq!(briefing.mentions[1].kind, MentionKind::DirectMention);
        assert_eq!(briefing.mentions[1].channel_label, "#eng");

        // Unread channels: #eng (1 unread after last_read; own msg & reply excluded) and @Alice DM;
        // muted #noisy excluded.
        assert_eq!(briefing.unread_channels.len(), 2);
        assert!(
            briefing
                .unread_channels
                .iter()
                .all(|c| c.channel != ChannelId::from("C_MUTED"))
        );
        let eng = briefing
            .unread_channels
            .iter()
            .find(|c| c.channel == ChannelId::from("C_ENG"))
            .expect("eng unread");
        assert_eq!(eng.message_count, 1);
        assert_eq!(eng.mention_count, 1);
        assert_eq!(eng.excerpts.len(), 1);

        // Active threads: Release checklist thread with +1 new reply.
        assert_eq!(briefing.active_threads.len(), 1);
        assert_eq!(briefing.active_threads[0].new_reply_count, 1);
        assert_eq!(
            briefing.active_threads[0].root_excerpt,
            "Release checklist thread"
        );
        assert_eq!(
            briefing.active_threads[0].latest_reply_excerpt,
            "Tests are green now!"
        );

        // Transcripts: 1 file clip + 1 huddle.
        assert_eq!(briefing.transcripts.len(), 2);
        assert!(briefing.transcripts.iter().any(
            |t| t.kind == TranscriptKind::FileClip && t.file_id == Some(FileId::from("F_DEMO"))
        ));
        assert!(
            briefing
                .transcripts
                .iter()
                .any(|t| t.kind == TranscriptKind::Huddle && t.call_id.as_deref() == Some("CALL_1"))
        );

        let human = format_catchup_human(&briefing, false);
        assert!(human.contains("MENTIONS (2)"));
        assert!(human.contains("UNREAD CHANNELS (2)"));
        assert!(human.contains("ACTIVE THREADS (1)"));
        assert!(human.contains("TRANSCRIPTS (2)"));
    }

    #[test]
    fn catchup_since_last_visit_uses_recorded_timestamp_and_cross_workspace_merges() {
        let now = crate::types::time::now_epoch_secs();
        let mut s1 = Store::open_in_memory().unwrap();
        let mut s2 = Store::open_in_memory().unwrap();
        seed_test_workspace(&mut s1, "T1", "Acme", now);
        seed_test_workspace(&mut s2, "T2", "Beta", now);

        // Record a recent visit on T1 after all messages -> T1 should have 0 messages/mentions since visit.
        record_catchup_visit(&mut s1, &TeamId::from("T1"), now - 50).unwrap();
        assert_eq!(
            read_catchup_last_visit(&s1, &TeamId::from("T1")).unwrap(),
            Some(now - 50)
        );

        let targets = vec![
            WorkspaceCatchUpTarget {
                workspace: TeamId::from("T1"),
                workspace_name: Some("Acme".into()),
                self_user: Some(UserId::from("U_ME")),
                store: Arc::new(Mutex::new(s1)),
            },
            WorkspaceCatchUpTarget {
                workspace: TeamId::from("T2"),
                workspace_name: Some("Beta".into()),
                self_user: Some(UserId::from("U_ME")),
                store: Arc::new(Mutex::new(s2)),
            },
        ];

        let briefing = build_catchup_across_stores(
            &targets,
            CatchUpWindow::SinceLastVisit,
            now,
            DEFAULT_CATCHUP_LIMIT,
        )
        .unwrap();

        assert_eq!(briefing.workspaces.len(), 2);
        // Only T2's mentions/threads/unreads appear since T1 was visited at `now - 50`.
        assert!(
            briefing
                .mentions
                .iter()
                .all(|m| m.workspace == TeamId::from("T2"))
        );
        assert_eq!(briefing.mentions.len(), 2);
        assert!(
            briefing
                .active_threads
                .iter()
                .all(|t| t.workspace == TeamId::from("T2"))
        );

        let human = format_catchup_human(&briefing, true);
        assert!(human.contains("[Beta]"));
    }
}
