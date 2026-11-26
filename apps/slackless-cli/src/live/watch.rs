//! `watch`: live RTM event stream filtering, formatting, and local store mirroring.

use std::process::ExitCode;

use anyhow::Result;
use slackless_core::Event;
use slackless_core::rtm::{ConnectParams, ConnectionState, RtmClient, RtmConfig, RtmEvent};
use slackless_core::types::{ChannelId, MessageEvent, SlackEvent, TeamId};

use crate::archive::Archive;
use crate::output;
use crate::session::resolve_session;
use crate::{Ctx, fail};

use super::session::resolve_channel_with;

pub(crate) fn watch_reached_limit(count: Option<usize>, emitted: usize) -> bool {
    count.is_some_and(|max| emitted >= max)
}

/// `watch [--channel <ch>] [--count N]`: stream live Slack events over the RTM websocket,
/// mirroring incoming events into the local SQLite archive when present and exiting cleanly on Ctrl+C.
pub async fn run_watch(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: Option<&str>,
    count: Option<usize>,
) -> Result<ExitCode> {
    let session = match resolve_session(flag) {
        Ok(session) => session,
        Err(not_signed_in) => return Ok(fail(ctx, "not_signed_in", &not_signed_in.to_string())),
    };
    let mut archive = Archive::open_rw(&session.team_id).ok().flatten();
    let filter_ch =
        channel_arg.map(|ch| resolve_channel_with(archive.as_ref(), &session.team_id, ch).0);
    let mut cfg = RtmConfig::new(ConnectParams::new(session.xoxc_token.clone()));
    cfg.d_cookie = Some(session.d_cookie.clone());
    let (_rtm, mut rx, driver) = match RtmClient::new(cfg) {
        Ok(parts) => parts,
        Err(err) => return Ok(fail(ctx, "client_error", &format!("starting RTM: {err}"))),
    };
    let driver_task = tokio::spawn(driver);
    let mut emitted = 0usize;

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                break;
            }
            ev = rx.recv() => {
                match ev {
                    Some(RtmEvent::State(ConnectionState::Disconnected { reason })) => {
                        driver_task.abort();
                        return Ok(fail(ctx, "disconnected", &reason));
                    }
                    Some(RtmEvent::Event(slack_ev)) => {
                        let maybe_out = format_watch_event(&slack_ev, filter_ch.as_ref());
                        if let Some(archive) = archive.as_mut()
                            && let Some(err) = mirror_event(archive, &session.team_id, slack_ev)
                        {
                            // stderr: `--json` consumers read only events on stdout.
                            eprintln!(
                                "slackless: could not mirror the event into the local archive ({err})"
                            );
                        }
                        if let Some((json_val, human_line)) = maybe_out {
                            output::emit(ctx.json, &json_val, &human_line);
                            emitted += 1;
                            if watch_reached_limit(count, emitted) {
                                break;
                            }
                        }
                    }
                    Some(_) => {}
                    None => break,
                }
            }
        }
    }
    driver_task.abort();
    Ok(ExitCode::SUCCESS)
}

/// Mirrors one websocket event into the local archive and returns the store failure to report:
/// the reducer folds a failed write into [`Event::Error`] rather than failing, so ignoring its
/// answer loses the error silently.
pub(crate) fn mirror_event(
    archive: &mut Archive,
    workspace: &TeamId,
    ev: SlackEvent,
) -> Option<String> {
    match slackless_core::apply_event(archive.store_mut(), workspace, ev) {
        Some(Event::Error(err)) => Some(err),
        _ => None,
    }
}

pub(crate) fn format_watch_event(
    ev: &SlackEvent,
    filter_ch: Option<&ChannelId>,
) -> Option<(serde_json::Value, String)> {
    match ev {
        SlackEvent::Message(MessageEvent::Message(msg)) => {
            if let Some(target) = filter_ch
                && msg.channel.as_ref() != Some(target)
            {
                return None;
            }
            let ch = msg.channel.as_ref().map(|c| c.as_str()).unwrap_or("");
            let who = msg
                .user
                .as_ref()
                .map(|u| u.as_str())
                .or(msg.bot_id.as_ref().map(|b| b.as_str()))
                .unwrap_or("unknown");
            let json = serde_json::json!({
                "type": "message",
                "channel": ch,
                "ts": msg.ts.as_str(),
                "thread_ts": msg.thread_ts.as_ref().map(|t| t.as_str()),
                "user": msg.user.as_ref().map(|u| u.as_str()),
                "text": msg.text,
            });
            let human = format!("message\t{ch}\t{}\t{who}: {}\n", msg.ts, msg.text);
            Some((json, human))
        }
        SlackEvent::ReactionAdded(rx) => {
            if let Some(target) = filter_ch
                && rx.item.channel.as_ref() != Some(target)
            {
                return None;
            }
            let ch = rx.item.channel.as_ref().map(|c| c.as_str()).unwrap_or("");
            let ts = rx.item.ts.as_ref().map(|t| t.as_str()).unwrap_or("");
            let json = serde_json::json!({
                "type": "reaction_added",
                "channel": ch,
                "ts": ts,
                "user": rx.user.as_str(),
                "reaction": rx.reaction,
            });
            let human = format!(
                "reaction_added\t{ch}\t{ts}\t{} :{}:\n",
                rx.user, rx.reaction
            );
            Some((json, human))
        }
        SlackEvent::ThreadMarked(tm) => {
            let sub = &tm.subscription;
            if let Some(target) = filter_ch
                && &sub.channel != target
            {
                return None;
            }
            let last_read = sub.last_read.as_ref().map(|t| t.as_str());
            let json = serde_json::json!({
                "type": "thread_marked",
                "channel": sub.channel.as_str(),
                "thread_ts": sub.thread_ts.as_str(),
                "last_read": last_read,
                "active": sub.active,
            });
            let human = format!(
                "thread_marked\t{}\t{}\tlast_read={}\n",
                sub.channel,
                sub.thread_ts,
                last_read.unwrap_or("-")
            );
            Some((json, human))
        }
        _ => None,
    }
}
