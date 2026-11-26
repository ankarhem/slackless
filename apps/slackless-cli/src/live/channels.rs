//! `unread` and `backfill` live channel commands.

use std::collections::HashMap;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result};
use slackless_core::api::{
    ApiError, ClientCounts, ConversationCounts, HistoryOpts, ListOpts, Page, SlackApi,
};
use slackless_core::types::{ChannelId, Message, TeamId, Ts};
use slackless_core::{NotifyLevel, Synced, read_synced_spans, write_synced_spans};

use crate::Ctx;
use crate::archive::Archive;
use crate::output;

use super::session::{api_fail, api_for, display_channel, resolve_channel_with};

/// Maximum 429 rate-limit waits per history page (aligned with `slackless_core::backfill`).
pub(crate) const MAX_RATE_LIMIT_WAITS: u32 = 5;
/// Polite Tier-3 delay between `conversations.history` pages.
const BACKFILL_PAGE_DELAY: Duration = Duration::from_millis(1200);

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

pub(crate) fn clamp_backfill_page_size(page_size: u32) -> u32 {
    page_size.clamp(1, 200)
}

pub(crate) fn backfill_should_stop(max_pages: usize, pages_fetched: usize) -> bool {
    max_pages > 0 && pages_fetched >= max_pages
}

/// Returns the next pagination cursor if non-empty and strictly different from `current`
/// (stuck-cursor guard matching `slackless_core::backfill`).
pub(crate) fn next_backfill_cursor(current: Option<&str>, next: Option<String>) -> Option<String> {
    next.filter(|c| !c.is_empty() && current != Some(c.as_str()))
}

pub(crate) fn format_backfill_output(
    workspace: &TeamId,
    channel: &ChannelId,
    label: Option<&str>,
    pages_fetched: usize,
    messages_stored: usize,
    reached_end: bool,
) -> (serde_json::Value, String) {
    let display = display_channel(channel, label);
    let json = serde_json::json!({
        "ok": true,
        "workspace": workspace.as_str(),
        "channel": channel.as_str(),
        "label": display,
        "pages": pages_fetched,
        "messages": messages_stored,
        "reached_end": reached_end,
    });
    let human = format!(
        "backfilled {display}: {messages_stored} messages across {pages_fetched} page(s) (complete={reached_end})\n"
    );
    (json, human)
}

/// Per-channel summary emitted by `slackless archive backfill`.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct ArchiveBackfillChannelSummary {
    pub channel: String,
    pub label: String,
    pub pages: usize,
    pub messages: usize,
    pub threads: usize,
    pub replies: usize,
    pub reached_end: bool,
}

pub(crate) fn format_archive_backfill_output(
    workspace: &TeamId,
    include_threads: bool,
    channels: &[ArchiveBackfillChannelSummary],
) -> (serde_json::Value, String) {
    let total_pages: usize = channels.iter().map(|c| c.pages).sum();
    let total_messages: usize = channels.iter().map(|c| c.messages).sum();
    let total_threads: usize = channels.iter().map(|c| c.threads).sum();
    let total_replies: usize = channels.iter().map(|c| c.replies).sum();
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

fn record_synced_page(
    spans: &mut HashMap<ChannelId, Synced>,
    channel: &ChannelId,
    items: &[Message],
    reached_end: bool,
) {
    let page_newest = items.iter().map(|m| &m.ts).max().cloned();
    let page_oldest = items.iter().map(|m| &m.ts).min().cloned();
    match spans.get_mut(channel) {
        Some(span) => {
            if let Some(newest) = page_newest
                && newest > span.newest
            {
                span.newest = newest;
            }
            if let Some(oldest) = page_oldest
                && oldest < span.oldest
            {
                span.oldest = oldest;
            }
            span.complete |= reached_end;
        }
        None => {
            if let (Some(newest), Some(oldest)) = (page_newest, page_oldest) {
                spans.insert(
                    channel.clone(),
                    Synced {
                        newest,
                        oldest,
                        complete: reached_end,
                    },
                );
            } else if reached_end {
                spans.insert(
                    channel.clone(),
                    Synced {
                        newest: Ts::from("0"),
                        oldest: Ts::from("0"),
                        complete: true,
                    },
                );
            }
        }
    }
}

enum BackfillStepError {
    Api(ApiError),
    Store(anyhow::Error),
}

#[allow(clippy::too_many_arguments)]
async fn backfill_one_channel(
    api: &SlackApi,
    archive: &mut Archive,
    workspace: &TeamId,
    channel: &ChannelId,
    label: Option<&str>,
    include_threads: bool,
    max_pages: usize,
    page_size: u32,
    spans: &mut HashMap<ChannelId, Synced>,
) -> std::result::Result<ArchiveBackfillChannelSummary, BackfillStepError> {
    let opts = HistoryOpts {
        limit: Some(clamp_backfill_page_size(page_size)),
        ..Default::default()
    };
    let mut cursor: Option<String> = None;
    let mut pages_fetched: usize = 0;
    let mut messages_stored: usize = 0;
    let mut threads_fetched: usize = 0;
    let mut replies_stored: usize = 0;
    let mut reached_end = false;

    loop {
        if backfill_should_stop(max_pages, pages_fetched) {
            break;
        }
        let mut waits = 0u32;
        let page = loop {
            match api
                .conversations_history(channel, &opts, cursor.clone())
                .await
            {
                Ok(page) => break page,
                Err(ApiError::RateLimited { retry_after }) if waits < MAX_RATE_LIMIT_WAITS => {
                    waits += 1;
                    tokio::time::sleep(retry_after.max(Duration::from_secs(1))).await;
                }
                Err(err) => return Err(BackfillStepError::Api(err)),
            }
        };
        pages_fetched += 1;
        let next = next_backfill_cursor(cursor.as_deref(), page.next_cursor);
        let page_done = next.is_none();
        if !page.items.is_empty() {
            archive
                .store_mut()
                .upsert_messages(workspace, channel, &page.items)
                .with_context(|| format!("storing history page for {channel}"))
                .map_err(BackfillStepError::Store)?;
            messages_stored += page.items.len();
        }
        record_synced_page(spans, channel, &page.items, page_done);
        write_synced_spans(archive.store_mut(), workspace, spans)
            .with_context(|| format!("persisting backfill progress for {workspace}"))
            .map_err(BackfillStepError::Store)?;

        if include_threads {
            let thread_roots: Vec<Ts> = page
                .items
                .iter()
                .filter(|m| m.reply_count.unwrap_or(0) > 0)
                .map(|m| m.ts.clone())
                .collect();
            for root_ts in thread_roots {
                let mut reply_cursor: Option<String> = None;
                let mut got_any = false;
                loop {
                    let mut reply_waits = 0u32;
                    let reply_page = loop {
                        match api
                            .conversations_replies(
                                channel,
                                &root_ts,
                                &HistoryOpts::default(),
                                reply_cursor.clone(),
                            )
                            .await
                        {
                            Ok(rp) => break rp,
                            Err(ApiError::RateLimited { retry_after })
                                if reply_waits < MAX_RATE_LIMIT_WAITS =>
                            {
                                reply_waits += 1;
                                tokio::time::sleep(retry_after.max(Duration::from_secs(1))).await;
                            }
                            Err(err) if err.is_auth_expired() => {
                                return Err(BackfillStepError::Api(err));
                            }
                            Err(_) => {
                                break Page {
                                    items: Vec::new(),
                                    next_cursor: None,
                                };
                            }
                        }
                    };
                    if !reply_page.items.is_empty() {
                        got_any = true;
                        let reply_count =
                            reply_page.items.iter().filter(|m| m.ts != root_ts).count();
                        archive
                            .store_mut()
                            .upsert_messages(workspace, channel, &reply_page.items)
                            .with_context(|| {
                                format!("storing thread replies for {channel} ({root_ts})")
                            })
                            .map_err(BackfillStepError::Store)?;
                        replies_stored += reply_count;
                    }
                    match next_backfill_cursor(reply_cursor.as_deref(), reply_page.next_cursor) {
                        Some(next_rc) => {
                            reply_cursor = Some(next_rc);
                            tokio::time::sleep(BACKFILL_PAGE_DELAY).await;
                        }
                        None => break,
                    }
                }
                if got_any {
                    threads_fetched += 1;
                }
            }
        }

        match next {
            Some(next_cursor) => {
                cursor = Some(next_cursor);
                tokio::time::sleep(BACKFILL_PAGE_DELAY).await;
            }
            None => {
                reached_end = true;
                break;
            }
        }
    }

    Ok(ArchiveBackfillChannelSummary {
        channel: channel.as_str().to_owned(),
        label: display_channel(channel, label),
        pages: pages_fetched,
        messages: messages_stored,
        threads: threads_fetched,
        replies: replies_stored,
        reached_end,
    })
}

/// `backfill <CHANNEL> [--pages N] [--page-size N]`
/// Walks `conversations.history` pages politely (Tier-3 pacing, bounded 429 waits, stuck-cursor
/// guard) and batch-upserts each page into the local SQLite archive in one transaction (`X-1`).
pub async fn run_backfill(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    max_pages: usize,
    page_size: u32,
) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let mut archive = Archive::open_or_create_rw(&session.workspace)?;
    let (channel, label) = resolve_channel_with(Some(&archive), &session.workspace, channel_arg);
    let mut spans = read_synced_spans(archive.store(), &session.workspace);
    let summary = match backfill_one_channel(
        &session.api,
        &mut archive,
        &session.workspace,
        &channel,
        label.as_deref(),
        false,
        max_pages,
        page_size,
        &mut spans,
    )
    .await
    {
        Ok(summary) => summary,
        Err(BackfillStepError::Api(err)) => return Ok(api_fail(ctx, &err)),
        Err(BackfillStepError::Store(err)) => return Err(err),
    };

    let (json, human) = format_backfill_output(
        &session.workspace,
        &channel,
        label.as_deref(),
        summary.pages,
        summary.messages,
        summary.reached_end,
    );
    output::emit(ctx.json, &json, &human);
    Ok(ExitCode::SUCCESS)
}

/// `archive backfill [<CHANNEL> | --all] [--threads] [--pages N] [--page-size N]`
/// Walks `conversations.history` (and optionally `conversations.replies`) for a single channel
/// or every member conversation in the workspace, persisting `backfill_synced_spans` in SQLite.
pub async fn run_archive_backfill(
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
    let mut spans = read_synced_spans(archive.store(), &session.workspace);

    let targets: Vec<(ChannelId, Option<String>)> = if let Some(arg) = channel_arg
        && !all
    {
        vec![resolve_channel_with(
            Some(&archive),
            &session.workspace,
            arg,
        )]
    } else {
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
        if stored.is_empty() {
            // Fresh archive: pull conversation list from Slack once so `--all` has targets.
            let mut cursor: Option<String> = None;
            let mut convs = Vec::new();
            for _ in 0..10 {
                match session
                    .api
                    .conversations_list(&ListOpts::default(), cursor.clone())
                    .await
                {
                    Ok(page) => {
                        convs.extend(page.items);
                        match next_backfill_cursor(cursor.as_deref(), page.next_cursor) {
                            Some(next) => cursor = Some(next),
                            None => break,
                        }
                    }
                    Err(err) => return Ok(api_fail(ctx, &err)),
                }
            }
            if !convs.is_empty() {
                archive
                    .store_mut()
                    .upsert_conversations(&session.workspace, &convs)
                    .with_context(|| format!("storing conversations for {}", session.workspace))?;
                stored = convs
                    .into_iter()
                    .filter(|c| (c.is_member || c.is_im || c.is_mpim) && !c.is_archived)
                    .map(|c| {
                        let name = c.name.as_deref().map(|n| format!("#{n}"));
                        (c.id, name)
                    })
                    .collect();
            }
        }
        stored
    };

    let mut summaries = Vec::with_capacity(targets.len());
    for (channel, label) in targets {
        match backfill_one_channel(
            &session.api,
            &mut archive,
            &session.workspace,
            &channel,
            label.as_deref(),
            include_threads,
            max_pages,
            page_size,
            &mut spans,
        )
        .await
        {
            Ok(summary) => summaries.push(summary),
            Err(BackfillStepError::Api(err)) => return Ok(api_fail(ctx, &err)),
            Err(BackfillStepError::Store(err)) => return Err(err),
        }
    }

    let (json, human) =
        format_archive_backfill_output(&session.workspace, include_threads, &summaries);
    output::emit(ctx.json, &json, &human);
    Ok(ExitCode::SUCCESS)
}
