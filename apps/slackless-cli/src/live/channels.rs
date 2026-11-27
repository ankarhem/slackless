//! `unread` and `backfill` live channel commands.

use std::collections::HashMap;
use std::process::ExitCode;

use anyhow::Result;
use slackless_core::api::{
    ApiError, ClientCounts, ConversationCounts, HistoryOpts, ListOpts, Page, SlackApi,
};
use slackless_core::backfill::{
    HistorySink, HistoryWalk, PAGE_DELAY, WalkPage, wait_out_rate_limits, walk_history,
};
use slackless_core::types::{ChannelId, Message, TeamId, Ts};
use slackless_core::{CoreError, NotifyLevel, Synced, read_synced_spans, write_synced_spans};

use crate::Ctx;
use crate::archive::Archive;
use crate::output;

use super::session::{
    LiveSession, api_fail, api_for, classify_api_error, display_channel, resolve_channel_with,
};
use crate::commands::CmdError;
use crate::fail;

pub(crate) struct UnreadSummary {
    pub items: Vec<serde_json::Value>,
    pub human: String,
    pub total_mentions: u32,
    pub threads_unread: bool,
}

/// Filter and format `client.counts` unread entries, suppressing muted conversations with 0 mentions.
pub(crate) fn summarize_unread<F>(
    counts: &ClientCounts,
    notify_levels: &HashMap<ChannelId, NotifyLevel>,
    label_for: F,
) -> UnreadSummary
where
    F: Fn(&ChannelId) -> String,
{
    let mut items = Vec::new();
    let mut lines = String::new();
    let mut total_mentions: u32 = 0;

    let groups: [(&str, &[ConversationCounts]); 3] = [
        ("dm", &counts.ims),
        ("mpim", &counts.mpims),
        ("channel", &counts.channels),
    ];
    for (kind, list) in groups {
        for c in list.iter().filter(|c| c.is_unread()) {
            let mentions = c.effective_mentions();
            if notify_levels.get(&c.id) == Some(&NotifyLevel::Muted) && mentions == 0 {
                continue;
            }
            total_mentions = total_mentions.saturating_add(mentions);
            let label = label_for(&c.id);
            lines.push_str(&format!(
                "{kind}\t{}\t{label}\tmentions={mentions}\n",
                c.id.as_str()
            ));
            items.push(serde_json::json!({
                "id": c.id.as_str(),
                "label": label,
                "kind": kind,
                "mentions": mentions,
                "has_unreads": c.has_unreads,
                "latest": c.latest.as_ref().map(|t| t.as_str()),
            }));
        }
    }
    let threads_unread = counts.threads.has_unreads || counts.threads.mention_count > 0;
    if items.is_empty() && !threads_unread {
        lines.push_str("all caught up\n");
    } else if threads_unread {
        lines.push_str(&format!(
            "threads\t-\tThreads\tmentions={}\n",
            counts.threads.mention_count
        ));
    }
    UnreadSummary {
        items,
        human: lines,
        total_mentions,
        threads_unread,
    }
}

pub(crate) fn format_unread_json(
    workspace: &TeamId,
    summary: &UnreadSummary,
    threads_mentions: u32,
) -> serde_json::Value {
    serde_json::json!({
        "ok": true,
        "workspace": workspace.as_str(),
        "unread_conversations": summary.items.len(),
        "total_mentions": summary.total_mentions,
        "threads_unread": summary.threads_unread,
        "threads_mentions": threads_mentions,
        "items": summary.items,
    })
}

/// `unread`: live unread channels, DMs, mentions, and threads (`client.counts`).
pub async fn run_unread(ctx: &Ctx, flag: Option<&str>) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let counts = match session.api.client_counts().await {
        Ok(c) => c,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };
    let archive = Archive::open(&session.workspace).ok().flatten();
    let notify_levels = archive
        .as_ref()
        .and_then(|a| a.notify_levels(&session.workspace).ok())
        .unwrap_or_default();
    // Batch-load all conversation labels in one query instead of N+1 `resolve_channel` calls.
    let channel_names = archive
        .as_ref()
        .and_then(|a| a.channel_names(&session.workspace).ok())
        .unwrap_or_default();
    let label_for = |id: &ChannelId| -> String {
        channel_names
            .get(id.as_str())
            .cloned()
            .unwrap_or_else(|| id.to_string())
    };
    let summary = summarize_unread(&counts, &notify_levels, label_for);
    let json = format_unread_json(&session.workspace, &summary, counts.threads.mention_count);
    output::emit(ctx.json, &json, &summary.human);
    Ok(ExitCode::SUCCESS)
}

/// Returns the next pagination cursor if non-empty and strictly different from `current`
/// (stuck-cursor guard matching `slackless_core::backfill`).
pub(crate) fn next_backfill_cursor(current: Option<&str>, next: Option<String>) -> Option<String> {
    next.filter(|c| !c.is_empty() && current != Some(c.as_str()))
}

/// One channel's outcome of a `backfill` run.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct BackfillChannelSummary {
    pub channel: String,
    pub label: String,
    pub pages: u64,
    pub messages: u64,
    pub threads: u64,
    pub replies: u64,
    /// The channel's history is in the archive back to its beginning.
    pub reached_end: bool,
}

pub(crate) fn format_backfill_output(
    workspace: &TeamId,
    include_threads: bool,
    channels: &[BackfillChannelSummary],
) -> (serde_json::Value, String) {
    let total_pages: u64 = channels.iter().map(|c| c.pages).sum();
    let total_messages: u64 = channels.iter().map(|c| c.messages).sum();
    let total_threads: u64 = channels.iter().map(|c| c.threads).sum();
    let total_replies: u64 = channels.iter().map(|c| c.replies).sum();
    let completed: usize = channels.iter().filter(|c| c.reached_end).count();

    let mut human = String::new();
    for ch in channels {
        let thread_note = if include_threads {
            format!(", {} replies across {} thread(s)", ch.replies, ch.threads)
        } else {
            String::new()
        };
        human.push_str(&format!(
            "backfilled {}: {} messages across {} page(s){thread_note} (complete={})\n",
            ch.label, ch.messages, ch.pages, ch.reached_end
        ));
    }
    if channels.len() > 1 {
        human.push_str(&format!(
            "total ({}): {completed}/{} channels complete, {total_messages} messages, {total_replies} thread replies across {total_pages} page(s)\n",
            workspace.as_str(),
            channels.len()
        ));
    } else if channels.is_empty() {
        human.push_str(&format!(
            "no channels found to backfill in workspace {}\n",
            workspace.as_str()
        ));
    }

    let json = serde_json::json!({
        "ok": true,
        "workspace": workspace.as_str(),
        "include_threads": include_threads,
        "channels_count": channels.len(),
        "completed_channels": completed,
        "pages": total_pages,
        "messages": total_messages,
        "threads": total_threads,
        "replies": total_replies,
        "channels": channels,
    });
    (json, human)
}

/// The CLI's side of a headless history walk: pages from `api`, one archive to store them in,
/// the synced spans of every channel walked so far, and Slack's Tier-3 pacing.
struct CliSink<'a> {
    api: &'a SlackApi,
    archive: &'a mut Archive,
    workspace: &'a TeamId,
    channel: &'a ChannelId,
    spans: &'a mut HashMap<ChannelId, Synced>,
    history_opts: HistoryOpts,
}

impl HistorySink for CliSink<'_> {
    async fn fetch(
        &mut self,
        latest: Option<Ts>,
        cursor: Option<String>,
    ) -> Result<Page<Message>, ApiError> {
        let opts = HistoryOpts {
            latest,
            ..self.history_opts.clone()
        };
        wait_out_rate_limits(|| {
            self.api
                .conversations_history(self.channel, &opts, cursor.clone())
        })
        .await
    }

    async fn fetch_replies(
        &mut self,
        thread_ts: Ts,
        cursor: Option<String>,
    ) -> Result<Page<Message>, ApiError> {
        let opts = HistoryOpts::default();
        wait_out_rate_limits(|| {
            self.api
                .conversations_replies(self.channel, &thread_ts, &opts, cursor.clone())
        })
        .await
    }

    async fn store(&mut self, items: Vec<Message>, fetched_at: i64) -> Result<(), CoreError> {
        self.archive.store_mut().upsert_history_page(
            self.workspace,
            self.channel,
            &items,
            fetched_at,
        )?;
        Ok(())
    }

    async fn store_replies(
        &mut self,
        items: Vec<Message>,
        fetched_at: i64,
    ) -> Result<(), CoreError> {
        self.archive.store_mut().upsert_replies_page(
            self.workspace,
            self.channel,
            &items,
            fetched_at,
        )?;
        Ok(())
    }

    async fn record_span(&mut self, span: &Synced) -> Result<(), CoreError> {
        self.spans.insert(self.channel.clone(), span.clone());
        write_synced_spans(self.archive.store_mut(), self.workspace, self.spans)?;
        Ok(())
    }

    async fn on_page(&mut self, _page: &WalkPage<'_>) -> Result<(), CoreError> {
        Ok(())
    }

    async fn await_resume(&mut self) -> Result<(), CoreError> {
        Ok(())
    }

    async fn pace(&mut self) -> Result<(), CoreError> {
        tokio::time::sleep(PAGE_DELAY).await;
        Ok(())
    }
}

/// Every member channel and DM of the workspace, pulling the conversation list from Slack once
/// when the archive knows none yet.
async fn backfill_targets(
    session: &LiveSession,
    archive: &mut Archive,
) -> Result<Vec<(ChannelId, Option<String>)>, CmdError> {
    let mut stored: Vec<(ChannelId, Option<String>)> = archive
        .channels(&session.workspace)
        .unwrap_or_default()
        .into_iter()
        .filter(|c| c.is_member && !c.is_archived)
        .map(|c| {
            let lbl = c.label();
            (c.id, Some(lbl))
        })
        .collect();
    for dm in archive.dms(&session.workspace).unwrap_or_default() {
        let lbl = dm.label();
        stored.push((dm.id, Some(lbl)));
    }
    if !stored.is_empty() {
        return Ok(stored);
    }
    // Fresh archive: pull every conversation kind from Slack so `--all` covers public/private
    // channels, DMs, and group DMs without silent truncation at a small page cap.
    let opts = ListOpts {
        kinds: vec![
            slackless_core::types::ConversationKind::PublicChannel,
            slackless_core::types::ConversationKind::PrivateChannel,
            slackless_core::types::ConversationKind::Im,
            slackless_core::types::ConversationKind::Mpim,
        ],
        exclude_archived: true,
        limit: Some(200),
    };
    let mut cursor: Option<String> = None;
    let mut seen_cursors = std::collections::HashSet::new();
    let mut convs = Vec::new();
    for _ in 0..100 {
        let page = match session.api.conversations_list(&opts, cursor.clone()).await {
            Ok(page) => page,
            Err(err) => {
                let (code, msg) = classify_api_error(&err);
                return Err(CmdError::new(code, msg));
            }
        };
        convs.extend(page.items);
        match next_backfill_cursor(cursor.as_deref(), page.next_cursor) {
            Some(next) if seen_cursors.insert(next.clone()) => cursor = Some(next),
            _ => break,
        }
    }
    if convs.is_empty() {
        return Ok(stored);
    }
    archive
        .store_mut()
        .upsert_conversations(&session.workspace, &convs)
        .map_err(|err| {
            CmdError::store(
                err,
                format!("storing conversations for {}", session.workspace),
            )
        })?;
    Ok(convs
        .into_iter()
        .filter(|c| (c.is_member || c.is_im || c.is_mpim) && !c.is_archived)
        .map(|c| {
            let name = c.name.as_deref().map(|n| format!("#{n}"));
            (c.id, name)
        })
        .collect())
}

/// `backfill [<CHANNEL> | --all] [--threads] [--pages N] [--page-size N]`
/// Walks `conversations.history` (and optionally `conversations.replies`) for one channel or
/// every member conversation in the workspace through the core walk: pages inside a synced span
/// are skipped, and every page is committed before the next is fetched.
pub async fn run_backfill(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: Option<&str>,
    all: bool,
    include_threads: bool,
    max_pages: usize,
    page_size: u32,
) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let mut archive = Archive::open_or_create_rw(&session.workspace)?;
    let workspace = session.workspace.clone();
    let mut spans = read_synced_spans(archive.store(), &workspace).unwrap_or_else(|err| {
        eprintln!(
            "slackless: could not read the stored backfill progress of {workspace} ({err}); walking from the newest page"
        );
        HashMap::new()
    });
    let targets: Vec<(ChannelId, Option<String>)> = match channel_arg {
        Some(arg) if !all => vec![resolve_channel_with(Some(&archive), &workspace, arg)],
        _ => match backfill_targets(&session, &mut archive).await {
            Ok(targets) => targets,
            Err(err) => return Ok(fail(ctx, err.code, &err.message)),
        },
    };
    let history_opts = HistoryOpts {
        limit: Some(page_size),
        ..HistoryOpts::default()
    };
    let walk = HistoryWalk {
        include_threads,
        max_pages,
    };
    let mut summaries = Vec::with_capacity(targets.len());
    for (channel, label) in targets {
        let prior = spans.get(&channel).cloned();
        let mut sink = CliSink {
            api: &session.api,
            archive: &mut archive,
            workspace: &workspace,
            channel: &channel,
            spans: &mut spans,
            history_opts: history_opts.clone(),
        };
        let stats = match walk_history(prior, walk, &mut sink).await {
            Ok(stats) => stats,
            Err(CoreError::Api(err)) => return Ok(api_fail(ctx, &err)),
            Err(CoreError::Store(err)) => {
                return Err(anyhow::Error::new(err).context("storing the backfilled history"));
            }
            Err(CoreError::Cancelled) => return Err(anyhow::anyhow!("the backfill was cancelled")),
        };
        summaries.push(BackfillChannelSummary {
            channel: channel.as_str().to_owned(),
            label: display_channel(&channel, label.as_deref()),
            pages: stats.pages_fetched,
            messages: stats.messages_stored,
            threads: stats.threads_fetched,
            replies: stats.replies_stored,
            reached_end: stats.span.as_ref().is_some_and(|span| span.complete),
        });
    }

    let (json, human) = format_backfill_output(&workspace, include_threads, &summaries);
    output::emit(ctx.json, &json, &human);
    Ok(ExitCode::SUCCESS)
}
