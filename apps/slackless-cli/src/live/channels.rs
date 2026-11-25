//! `unread` and `backfill` live channel commands.

use std::collections::HashMap;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Result;
use slackless_core::NotifyLevel;
use slackless_core::api::{ApiError, ClientCounts, ConversationCounts, HistoryOpts};
use slackless_core::types::{ChannelId, TeamId};

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
    let opts = HistoryOpts {
        limit: Some(clamp_backfill_page_size(page_size)),
        ..Default::default()
    };
    let mut cursor: Option<String> = None;
    let mut pages_fetched: usize = 0;
    let mut messages_stored: usize = 0;
    let mut reached_end = false;

    loop {
        if backfill_should_stop(max_pages, pages_fetched) {
            break;
        }
        let mut waits = 0u32;
        let page = loop {
            match session
                .api
                .conversations_history(&channel, &opts, cursor.clone())
                .await
            {
                Ok(page) => break page,
                Err(ApiError::RateLimited { retry_after }) if waits < MAX_RATE_LIMIT_WAITS => {
                    waits += 1;
                    tokio::time::sleep(retry_after.max(Duration::from_secs(1))).await;
                }
                Err(err) => return Ok(api_fail(ctx, &err)),
            }
        };
        pages_fetched += 1;
        archive
            .store_mut()
            .upsert_messages(&session.workspace, &channel, &page.items)?;
        messages_stored += page.items.len();
        match next_backfill_cursor(cursor.as_deref(), page.next_cursor) {
            Some(next) => {
                cursor = Some(next);
                tokio::time::sleep(BACKFILL_PAGE_DELAY).await;
            }
            None => {
                reached_end = true;
                break;
            }
        }
    }

    let (json, human) = format_backfill_output(
        &session.workspace,
        &channel,
        label.as_deref(),
        pages_fetched,
        messages_stored,
        reached_end,
    );
    output::emit(ctx.json, &json, &human);
    Ok(ExitCode::SUCCESS)
}
