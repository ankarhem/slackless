//! slackless CLI — headless search/read/write for agents (extra-features X-3).
//!
//! Local commands (`search`, `read`, `channels`, `dms`, `auth status`) run offline against the
//! durable store; `send`/`reply`/`react` go through `slackless-core`'s API client with a stored
//! session and print a clear "not signed in" message when none exists. `auth login` opens the
//! sign-in window; `auth logout` forgets stored sessions. Every command prints
//! `--json` for machines and a tab-separated human default. Nothing here needs a live Slack
//! connection to start.

mod archive;
mod cli;
mod commands;
mod live;
mod mcp;
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
    ArchiveCommand, AuthCommand, CanvasCommand, Cli, Commands, HuddleCommand, ListCommand,
    SavedCommand, StatusCommand,
};
pub(crate) use crate::cli::{CLI_VERSION, CanvasAccessArg, ListReadFormat};
use crate::commands::auth::{cmd_auth_login, cmd_auth_logout, cmd_auth_status};
use crate::commands::browse::{
    cmd_channels, cmd_digest, cmd_dms, cmd_revisions, cmd_threads, cmd_users,
};
use crate::commands::maintenance::{cmd_archive_status, cmd_stats, cmd_vacuum};
use crate::commands::messages::{
    ExportCmdOpts, SearchAllOpts, cmd_export, cmd_read, cmd_search, cmd_search_all_workspaces,
    cmd_thread,
};
use crate::live::{HuddleJoinArgs, HuddleTranscriptsArgs};
use crate::session::resolve_workspace;
use crate::write::{
    run_delete, run_edit, run_mark_read, run_pin, run_pins, run_react, run_reply, run_schedule,
    run_scheduled, run_send, run_thread_live, run_unreact, run_unschedule, run_upload,
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
    slackless_core::huddle::install_default_crypto_provider();
    let cli = Cli::parse();
    let ctx = Ctx { json: cli.json };
    let flag = cli.workspace.clone();
    let workspace = resolve_workspace(flag.as_deref());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    let result = match runtime {
        Ok(rt) => rt.block_on(dispatch(cli, &ctx, flag.as_deref(), workspace.as_ref())),
        Err(err) => Err(anyhow::anyhow!("starting the async runtime: {err}")),
    };
    result.unwrap_or_else(|err| fail(&ctx, "command_failed", &format!("{err:#}")))
}

async fn dispatch(
    cli: Cli,
    ctx: &Ctx,
    flag: Option<&str>,
    workspace: Option<&TeamId>,
) -> Result<ExitCode> {
    match cli.command {
        Commands::Search(args) if args.all_workspaces => cmd_search_all_workspaces(
            ctx,
            &args.query,
            SearchAllOpts {
                limit: args.limit,
                sort: args.sort.into(),
                format: args.format,
                out: args.out.as_deref(),
                include_deleted: args.include_deleted,
            },
        ),
        Commands::Search(args) => cmd_search(
            ctx,
            workspace,
            &args.query,
            args.limit,
            args.sort.into(),
            args.format,
            args.out.as_deref(),
            args.include_deleted,
        ),
        Commands::Read(args) => cmd_read(
            ctx,
            workspace,
            &args.channel,
            args.limit,
            args.since.as_deref(),
            args.include_deleted,
        ),
        Commands::Thread(args) if args.live => {
            run_thread_live(ctx, flag, &args.channel, &args.thread_ts, args.limit).await
        }
        Commands::Thread(args) => cmd_thread(ctx, workspace, &args.channel, &args.thread_ts),
        Commands::Send(args) => {
            run_send(ctx, flag, &args.channel, args.text, args.allow_secrets).await
        }
        Commands::Reply(args) => {
            run_reply(
                ctx,
                flag,
                &args.channel,
                &args.thread_ts,
                args.text,
                args.allow_secrets,
            )
            .await
        }
        Commands::Edit(args) => {
            run_edit(
                ctx,
                flag,
                &args.channel,
                &args.ts,
                args.text,
                args.allow_secrets,
            )
            .await
        }
        Commands::Delete { channel, ts } => run_delete(ctx, flag, &channel, &ts).await,
        Commands::React(args) => run_react(ctx, flag, &args.channel, &args.ts, &args.emoji).await,
        Commands::Unreact(args) => {
            run_unreact(ctx, flag, &args.channel, &args.ts, &args.emoji).await
        }
        Commands::Upload(args) => {
            run_upload(
                ctx,
                flag,
                &args.channel,
                &args.files,
                args.comment,
                args.thread_ts.as_deref(),
                args.allow_secrets,
            )
            .await
        }
        Commands::Channels => cmd_channels(ctx, workspace),
        Commands::Dms => cmd_dms(ctx, workspace),
        Commands::Users {
            query,
            include_deleted,
        } => cmd_users(ctx, workspace, query.as_deref(), include_deleted),
        Commands::Digest(args) => {
            cmd_digest(ctx, workspace, &args.since, args.all_workspaces, args.limit)
        }
        Commands::MarkRead { channel, ts } => {
            run_mark_read(ctx, flag, &channel, ts.as_deref()).await
        }
        Commands::Pins { channel } => run_pins(ctx, flag, &channel).await,
        Commands::Pin { channel, ts } => run_pin(ctx, flag, &channel, &ts, false).await,
        Commands::Unpin { channel, ts } => run_pin(ctx, flag, &channel, &ts, true).await,
        Commands::Schedule(args) => {
            run_schedule(
                ctx,
                flag,
                &args.channel,
                args.text,
                &args.at,
                args.thread_ts.as_deref(),
                args.allow_secrets,
            )
            .await
        }
        Commands::Scheduled { channel } => run_scheduled(ctx, flag, channel.as_deref()).await,
        Commands::Unschedule { channel, id } => run_unschedule(ctx, flag, &channel, &id).await,
        Commands::Stats { breakdown } => cmd_stats(ctx, workspace, breakdown),
        Commands::Vacuum => cmd_vacuum(ctx, workspace),
        Commands::Threads { channel, limit } => {
            cmd_threads(ctx, workspace, channel.as_deref(), limit)
        }
        Commands::Revisions { channel, ts } => cmd_revisions(ctx, workspace, &channel, &ts),
        Commands::Export(args) => cmd_export(
            ctx,
            workspace,
            ExportCmdOpts {
                channel: args.channel.as_deref(),
                thread_ts: args.thread.as_deref(),
                transcript_file_id: args.transcript.as_deref(),
                huddle_call_id: args.huddle.as_deref(),
                include_threads: args.include_threads,
                include_deleted: !args.exclude_deleted,
                format: args.format,
                limit: args.limit,
                out: args.out.as_deref(),
            },
        ),
        Commands::MarkThread(args) => {
            live::run_mark_thread(
                ctx,
                flag,
                &args.channel,
                &args.thread_ts,
                args.ts.as_deref(),
                args.unread,
            )
            .await
        }
        Commands::Saved { command } => dispatch_saved(ctx, flag, *command).await,
        Commands::Files(args) => {
            live::run_files(ctx, flag, args.channel.as_deref(), args.types, args.limit).await
        }
        Commands::Canvas { command } => dispatch_canvas(ctx, flag, *command).await,
        Commands::List { command } => dispatch_list(ctx, flag, *command).await,
        Commands::Watch { channel, count } => {
            live::run_watch(ctx, flag, channel.as_deref(), count).await
        }
        Commands::Huddle { command } => dispatch_huddle(ctx, flag, *command).await,
        Commands::Status { command } => dispatch_status(ctx, flag, *command).await,
        Commands::Unread => live::run_unread(ctx, flag).await,
        Commands::Download { file_id, out } => {
            live::run_download(ctx, flag, &file_id, out.as_deref()).await
        }
        Commands::Transcribe(args) => {
            live::run_transcribe(
                ctx,
                flag,
                workspace,
                &args.file_id,
                args.file.as_deref(),
                args.provider.as_deref(),
            )
            .await
        }
        Commands::Backfill(args) => {
            live::run_backfill(ctx, flag, &args.channel, args.pages, args.page_size).await
        }
        Commands::Archive { command } => match *command {
            ArchiveCommand::Backfill(args) => {
                live::run_archive_backfill(
                    ctx,
                    flag,
                    args.channel.as_deref(),
                    args.all,
                    args.threads,
                    args.pages,
                    args.page_size,
                )
                .await
            }
            ArchiveCommand::Status => cmd_archive_status(ctx, workspace),
        },
        Commands::Update {
            check,
            force,
            restart,
        } => update::run_update(ctx, check, force, restart).await,
        Commands::Mcp => mcp::run_mcp(flag).await,
        Commands::Auth { command } => dispatch_auth(ctx, flag, *command),
    }
}

async fn dispatch_saved(ctx: &Ctx, flag: Option<&str>, command: SavedCommand) -> Result<ExitCode> {
    match command {
        SavedCommand::List { filter, limit } => {
            live::run_saved_list(ctx, flag, &filter, limit).await
        }
        SavedCommand::Add { channel, ts, due } => {
            live::run_saved_add(ctx, flag, &channel, &ts, due).await
        }
        SavedCommand::Done { channel, ts } => live::run_saved_done(ctx, flag, &channel, &ts).await,
        SavedCommand::Delete { channel, ts } => {
            live::run_saved_delete(ctx, flag, &channel, &ts).await
        }
    }
}

async fn dispatch_canvas(
    ctx: &Ctx,
    flag: Option<&str>,
    command: CanvasCommand,
) -> Result<ExitCode> {
    match command {
        CanvasCommand::Read { file_id, html } => {
            live::run_canvas_read(ctx, flag, &file_id, html).await
        }
        CanvasCommand::Create {
            title,
            markdown,
            channel,
        } => live::run_canvas_create(ctx, flag, &title, markdown, channel.as_deref()).await,
        CanvasCommand::Edit {
            file_id,
            markdown,
            section,
            append,
        } => live::run_canvas_edit(ctx, flag, &file_id, markdown, section, append).await,
        CanvasCommand::Info { canvas_id } => live::run_canvas_info(ctx, flag, &canvas_id).await,
        CanvasCommand::Delete { canvas_id } => live::run_canvas_delete(ctx, flag, &canvas_id).await,
        CanvasCommand::Share {
            canvas_id,
            access,
            channels,
            users,
            post,
        } => live::run_canvas_share(ctx, flag, &canvas_id, access, &channels, &users, post).await,
        CanvasCommand::Unshare {
            canvas_id,
            channels,
            users,
        } => live::run_canvas_unshare(ctx, flag, &canvas_id, &channels, &users).await,
        CanvasCommand::Comments {
            canvas_id,
            resolved,
            all,
        } => live::run_canvas_comments(ctx, flag, &canvas_id, resolved, all).await,
        CanvasCommand::Comment {
            canvas_id,
            text,
            thread,
            allow_secrets,
        } => live::run_canvas_comment(ctx, flag, &canvas_id, text, thread, allow_secrets).await,
        CanvasCommand::Resolve {
            canvas_id,
            thread,
            all,
            reopen,
        } => live::run_canvas_resolve(ctx, flag, &canvas_id, thread, all, reopen).await,
    }
}

async fn dispatch_list(ctx: &Ctx, flag: Option<&str>, command: ListCommand) -> Result<ExitCode> {
    match command {
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
    }
}

async fn dispatch_huddle(
    ctx: &Ctx,
    flag: Option<&str>,
    command: HuddleCommand,
) -> Result<ExitCode> {
    match command {
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
            camera,
            blur,
            screen,
            transcribe,
        } => {
            live::run_huddle_join(
                ctx,
                flag,
                HuddleJoinArgs {
                    channel,
                    muted,
                    duration,
                    input,
                    output_dev: output,
                    react,
                    camera,
                    blur,
                    screen,
                    transcribe,
                },
            )
            .await
        }
        HuddleCommand::Transcripts {
            channel,
            search,
            lang,
            call_id,
            format,
            limit,
        } => live::run_huddle_transcripts(
            ctx,
            flag,
            HuddleTranscriptsArgs {
                channel,
                search,
                lang,
                call_id,
                format,
                limit,
            },
        ),
        HuddleCommand::Link { channel } => live::run_huddle_link(ctx, flag, &channel).await,
        HuddleCommand::Invite { channel, user } => {
            live::run_huddle_invite(ctx, flag, &channel, &user).await
        }
    }
}

async fn dispatch_status(
    ctx: &Ctx,
    flag: Option<&str>,
    command: StatusCommand,
) -> Result<ExitCode> {
    match command {
        StatusCommand::Show => live::run_status_show(ctx, flag).await,
        StatusCommand::Set {
            text,
            emoji,
            expires_in,
        } => live::run_status_set(ctx, flag, text, emoji, expires_in).await,
        StatusCommand::Clear => live::run_status_clear(ctx, flag).await,
    }
}

fn dispatch_auth(ctx: &Ctx, flag: Option<&str>, command: AuthCommand) -> Result<ExitCode> {
    match command {
        AuthCommand::Status => cmd_auth_status(ctx),
        AuthCommand::Login { domain } => cmd_auth_login(ctx, domain),
        AuthCommand::Logout => cmd_auth_logout(ctx, flag),
    }
}
