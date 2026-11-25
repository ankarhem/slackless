//! slackless CLI — headless search/read/write for agents (extra-features X-3).
//!
//! Local commands (`search`, `read`, `channels`, `dms`, `auth status`) run offline against the
//! durable store; `send`/`reply`/`react` go through `slackless-core`'s API client with a stored
//! session and print a clear "not signed in" message when none exists. `auth login` opens the
//! sign-in window; `auth logout` forgets stored sessions. Every command prints
//! `--json` for machines and a tab-separated human default. Nothing here needs a live Slack
//! connection to start.

#![allow(clippy::large_unopt_stack_frames)]

mod archive;
mod cli;
mod commands;
mod live;
mod output;
mod session;
#[cfg(test)]
mod tests;
mod update;
mod write;

use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;
use slackless_core::types::TeamId;

use crate::cli::{
    AuthCommand, CanvasCommand, Cli, Commands, HuddleCommand, ListCommand, SavedCommand,
    StatusCommand,
};
pub(crate) use crate::cli::{CLI_VERSION, ListReadFormat};
use crate::commands::auth::{cmd_auth_login, cmd_auth_logout, cmd_auth_status};
use crate::commands::browse::{
    cmd_channels, cmd_digest, cmd_dms, cmd_revisions, cmd_threads, cmd_users,
};
use crate::commands::maintenance::{cmd_stats, cmd_vacuum};
use crate::commands::messages::{
    cmd_export, cmd_read, cmd_search, cmd_search_all_workspaces, cmd_thread,
};
use crate::session::resolve_workspace;
use crate::write::{
    run_delete, run_edit, run_mark_read, run_pin, run_pins, run_react, run_reply, run_schedule,
    run_scheduled, run_send, run_thread_live, run_unreact, run_upload,
};

/// Shared output mode for one invocation.
pub(crate) struct Ctx {
    pub(crate) json: bool,
}

/// A user-actionable failure: message + JSON envelope, not an error dump.
pub(crate) fn fail(ctx: &Ctx, code: &str, message: &str) -> ExitCode {
    if ctx.json {
        output::emit_json(&serde_json::json!({"ok": false, "error": code, "message": message}));
    } else {
        eprintln!("slackless: {message}");
    }
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let ctx = Ctx { json: cli.json };
    let flag = cli.workspace.clone();
    let workspace = resolve_workspace(flag.as_deref());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    let result = match runtime {
        Ok(rt) => rt.block_on(dispatch(cli, &ctx, flag, workspace)),
        Err(err) => Err(anyhow::anyhow!("starting the async runtime: {err}")),
    };
    result.unwrap_or_else(|err| fail(&ctx, "command_failed", &format!("{err:#}")))
}

async fn dispatch(
    cli: Cli,
    ctx: &Ctx,
    flag: Option<String>,
    workspace: Option<TeamId>,
) -> Result<ExitCode> {
    let flag = flag.as_deref();
    match cli.command {
        Commands::Search {
            query,
            limit,
            all_workspaces,
            format,
            out,
        } => {
            if all_workspaces {
                cmd_search_all_workspaces(ctx, &query, limit)
            } else {
                cmd_search(
                    ctx,
                    workspace.as_ref(),
                    &query,
                    limit,
                    format,
                    out.as_deref(),
                )
            }
        }
        Commands::Read {
            channel,
            limit,
            since,
        } => cmd_read(ctx, workspace.as_ref(), &channel, limit, since.as_deref()),
        Commands::Thread {
            channel,
            thread_ts,
            live,
            limit,
        } => {
            if live {
                run_thread_live(ctx, flag, &channel, &thread_ts, limit).await
            } else {
                cmd_thread(ctx, workspace.as_ref(), &channel, &thread_ts)
            }
        }
        Commands::Send { channel, text } => run_send(ctx, flag, &channel, text).await,
        Commands::Reply {
            channel,
            thread_ts,
            text,
        } => run_reply(ctx, flag, &channel, &thread_ts, text).await,
        Commands::Edit { channel, ts, text } => run_edit(ctx, flag, &channel, &ts, text).await,
        Commands::Delete { channel, ts } => run_delete(ctx, flag, &channel, &ts).await,
        Commands::React { channel, ts, emoji } => run_react(ctx, flag, &channel, &ts, &emoji).await,
        Commands::Unreact { channel, ts, emoji } => {
            run_unreact(ctx, flag, &channel, &ts, &emoji).await
        }
        Commands::Upload {
            channel,
            files,
            comment,
            thread_ts,
        } => run_upload(ctx, flag, &channel, &files, comment, thread_ts.as_deref()).await,
        Commands::Channels => cmd_channels(ctx, workspace.as_ref()),
        Commands::Dms => cmd_dms(ctx, workspace.as_ref()),
        Commands::Users {
            query,
            include_deleted,
        } => cmd_users(ctx, workspace.as_ref(), query.as_deref(), include_deleted),
        Commands::Digest {
            since,
            all_workspaces,
            limit,
        } => cmd_digest(ctx, workspace.as_ref(), &since, all_workspaces, limit),
        Commands::MarkRead { channel, ts } => {
            run_mark_read(ctx, flag, &channel, ts.as_deref()).await
        }
        Commands::Pins { channel } => run_pins(ctx, flag, &channel).await,
        Commands::Pin { channel, ts } => run_pin(ctx, flag, &channel, &ts, false).await,
        Commands::Unpin { channel, ts } => run_pin(ctx, flag, &channel, &ts, true).await,
        Commands::Schedule {
            channel,
            text,
            at,
            thread_ts,
        } => run_schedule(ctx, flag, &channel, text, &at, thread_ts.as_deref()).await,
        Commands::Scheduled { channel } => run_scheduled(ctx, flag, channel.as_deref()).await,
        Commands::Stats { breakdown } => cmd_stats(ctx, workspace.as_ref(), breakdown),
        Commands::Vacuum => cmd_vacuum(ctx, workspace.as_ref()),
        Commands::Threads { channel, limit } => {
            cmd_threads(ctx, workspace.as_ref(), channel.as_deref(), limit)
        }
        Commands::Revisions { channel, ts } => {
            cmd_revisions(ctx, workspace.as_ref(), &channel, &ts)
        }
        Commands::Export {
            channel,
            format,
            limit,
            out,
        } => cmd_export(
            ctx,
            workspace.as_ref(),
            &channel,
            format,
            limit,
            out.as_deref(),
        ),
        Commands::MarkThread {
            channel,
            thread_ts,
            ts,
            unread,
        } => live::run_mark_thread(ctx, flag, &channel, &thread_ts, ts.as_deref(), unread).await,
        Commands::Saved { command } => match command {
            SavedCommand::List { filter, limit } => {
                live::run_saved_list(ctx, flag, &filter, limit).await
            }
            SavedCommand::Add { channel, ts, due } => {
                live::run_saved_add(ctx, flag, &channel, &ts, due).await
            }
            SavedCommand::Done { channel, ts } => {
                live::run_saved_done(ctx, flag, &channel, &ts).await
            }
            SavedCommand::Delete { channel, ts } => {
                live::run_saved_delete(ctx, flag, &channel, &ts).await
            }
        },
        Commands::Files {
            channel,
            types,
            limit,
        } => live::run_files(ctx, flag, channel.as_deref(), types, limit).await,
        Commands::Canvas { command } => match command {
            CanvasCommand::Read { file_id, html } => {
                live::run_canvas_read(ctx, flag, &file_id, html).await
            }
            CanvasCommand::Create {
                title,
                markdown,
                channel,
            } => {
                live::run_canvas_create(ctx, flag, &title, markdown.as_deref(), channel.as_deref())
                    .await
            }
            CanvasCommand::Edit {
                file_id,
                markdown,
                section,
                append,
            } => live::run_canvas_edit(ctx, flag, &file_id, markdown, section, append).await,
        },
        Commands::List { command } => match command {
            ListCommand::Read { list_id, format } => {
                live::run_list_read(ctx, flag, &list_id, format).await
            }
            ListCommand::Create {
                name,
                description,
                channel,
            } => {
                live::run_list_create(ctx, flag, &name, description.as_deref(), channel.as_deref())
                    .await
            }
            ListCommand::AddItem {
                list_id,
                title,
                column,
            } => live::run_list_add_item(ctx, flag, &list_id, title, column).await,
            ListCommand::DeleteItem { list_id, row_id } => {
                live::run_list_delete_item(ctx, flag, &list_id, &row_id).await
            }
            ListCommand::AddMessage {
                list_id,
                channel,
                ts,
            } => live::run_list_add_message(ctx, flag, &list_id, &channel, &ts).await,
        },
        Commands::Watch { channel, count } => {
            live::run_watch(ctx, flag, channel.as_deref(), count).await
        }
        Commands::Huddle { command } => match command {
            HuddleCommand::Devices => live::run_huddle_devices(ctx),
            HuddleCommand::Cameras => live::run_huddle_cameras(ctx),
            HuddleCommand::Screens => live::run_huddle_screens(ctx),
            HuddleCommand::Join {
                channel,
                muted,
                duration,
                input,
                output,
                react,
            } => {
                live::run_huddle_join(ctx, flag, &channel, muted, duration, input, output, react)
                    .await
            }
            HuddleCommand::Link { channel } => live::run_huddle_link(ctx, flag, &channel).await,
            HuddleCommand::Invite { channel, user } => {
                live::run_huddle_invite(ctx, flag, &channel, &user).await
            }
        },
        Commands::Status { command } => match command {
            StatusCommand::Show => live::run_status_show(ctx, flag).await,
            StatusCommand::Set {
                text,
                emoji,
                expires_in,
            } => live::run_status_set(ctx, flag, text, emoji, expires_in).await,
            StatusCommand::Clear => live::run_status_clear(ctx, flag).await,
        },
        Commands::Unread => live::run_unread(ctx, flag).await,
        Commands::Download { file_id, out } => {
            live::run_download(ctx, flag, &file_id, out.as_deref()).await
        }
        Commands::Backfill {
            channel,
            pages,
            page_size,
        } => live::run_backfill(ctx, flag, &channel, pages, page_size).await,
        Commands::Update {
            check,
            force,
            restart,
        } => update::run_update(ctx, check, force, restart),
        Commands::Auth { command } => match command {
            AuthCommand::Status => cmd_auth_status(ctx),
            AuthCommand::Login { domain } => cmd_auth_login(ctx, domain),
            AuthCommand::Logout => cmd_auth_logout(ctx, flag),
        },
    }
}
