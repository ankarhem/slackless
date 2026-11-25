//! `unread` and `backfill` live channel commands.

use std::collections::HashMap;
use std::process::ExitCode;

use anyhow::Result;
use slackless_core::NotifyLevel;
use slackless_core::api::{ApiError, ClientCounts, ConversationCounts, HistoryOpts};
use slackless_core::types::{ChannelId, TeamId};

use crate::archive::Archive;
use crate::output;
use crate::{Ctx, fail};

use super::session::{api_fail, api_for, display_channel, resolve_channel};

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
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let counts = match api.client_counts().await {
        Ok(c) => c,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };
    let archive = Archive::open(&workspace);
    let notify_levels = archive
        .as_ref()
        .map(|a| a.notify_levels(&workspace))
        .unwrap_or_default();
    let label_for = |id: &ChannelId| -> String {
        archive
            .as_ref()
            .and_then(|a| a.resolve_channel(&workspace, id.as_str()))
            .map(|r| r.label())
            .unwrap_or_else(|| id.to_string())
    };
    let summary = summarize_unread(&counts, &notify_levels, label_for);
    let json = format_unread_json(&workspace, &summary, counts.threads.mention_count);
    output::emit(ctx.json, &json, &summary.human);
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn clamp_backfill_page_size(page_size: u32) -> u32 {
    page_size.clamp(1, 200)
}

pub(crate) fn backfill_should_stop(max_pages: usize, pages_fetched: usize) -> bool {
    max_pages > 0 && pages_fetched >= max_pages
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
/// Walks `conversations.history` pages politely (Tier-3 pacing + 429 retry) and upserts every
/// fetched message into the local SQLite archive (`X-1`).
pub async fn run_backfill(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: &str,
    max_pages: usize,
    page_size: u32,
) -> Result<ExitCode> {
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let (channel, label) = resolve_channel(&workspace, channel_arg);
    let Some(path) = crate::archive::store_path(&workspace) else {
        return Ok(fail(
            ctx,
            "no_store_path",
            "could not resolve local archive path",
        ));
    };
    let mut store = slackless_store::Store::open(&path)?;
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
        let page = loop {
            match api
                .conversations_history(&channel, &opts, cursor.clone())
                .await
            {
                Ok(page) => break page,
                Err(ApiError::RateLimited { retry_after }) => {
                    tokio::time::sleep(retry_after.max(std::time::Duration::from_secs(1))).await;
                }
                Err(err) => return Ok(api_fail(ctx, &err)),
            }
        };
        pages_fetched += 1;
        for msg in &page.items {
            store.upsert_message(&workspace, &channel, msg)?;
            messages_stored += 1;
        }
        match page.next_cursor {
            Some(next) if !next.is_empty() => {
                cursor = Some(next);
                tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
            }
            _ => {
                reached_end = true;
                break;
            }
        }
    }

    let (json, human) = format_backfill_output(
        &workspace,
        &channel,
        label.as_deref(),
        pages_fetched,
        messages_stored,
        reached_end,
    );
    output::emit(ctx.json, &json, &human);
    Ok(ExitCode::SUCCESS)
}
