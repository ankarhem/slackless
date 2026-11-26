//! Session bootstrap, channel resolution, and API error mapping shared by all live & write commands.

use std::process::ExitCode;

use slackless_core::api::{ApiError, SlackApi};
use slackless_core::types::{ChannelId, TeamId, UserId};

use crate::archive::Archive;
use crate::session::resolve_session_from;
use crate::{Ctx, fail};

/// Authenticated live session client plus workspace and signed-in user identity.
pub(crate) struct LiveSession {
    pub(crate) api: SlackApi,
    pub(crate) workspace: TeamId,
    pub(crate) user_id: UserId,
}

/// Resolve credentials and construct a [`LiveSession`]; `Err` is the already-emitted CLI failure.
pub(crate) fn api_for(ctx: &Ctx, flag: Option<&str>) -> Result<LiveSession, ExitCode> {
    let session = match resolve_session_from(flag, ctx.sessions()) {
        Ok(session) => session,
        Err(not_signed_in) => return Err(fail(ctx, "not_signed_in", &not_signed_in.to_string())),
    };
    let api = SlackApi::new(
        &session.api_base(),
        session.xoxc_token.clone(),
        &session.d_cookie,
    )
    .map_err(|err| {
        fail(
            ctx,
            "client_error",
            &format!("building the Slack client: {err}"),
        )
    })?;
    Ok(LiveSession {
        api,
        workspace: session.team_id,
        user_id: session.user_id,
    })
}

/// Map an [`ApiError`] to its machine-readable CLI error code and human-facing message.
pub(crate) fn classify_api_error(err: &ApiError) -> (&'static str, String) {
    if err.is_auth_expired() {
        (
            "session_rejected",
            format!(
                "slack rejected the stored session ({err}); sign in again with `slackless auth login`"
            ),
        )
    } else {
        ("request_failed", err.to_string())
    }
}

pub(crate) fn api_fail(ctx: &Ctx, err: &ApiError) -> ExitCode {
    let (code, message) = classify_api_error(err);
    fail(ctx, code, &message)
}

/// Resolve a channel target against an optional archive, falling back to a `#`-stripped raw ID.
pub(crate) fn resolve_channel_with(
    archive: Option<&Archive>,
    workspace: &TeamId,
    arg: &str,
) -> (ChannelId, Option<String>) {
    match archive.and_then(|a| a.resolve_channel(workspace, arg).ok().flatten()) {
        Some(row) => {
            let label = row.label();
            (row.id, Some(label))
        }
        None => (ChannelId::from(arg.strip_prefix('#').unwrap_or(arg)), None),
    }
}

pub(crate) fn resolve_channel(workspace: &TeamId, arg: &str) -> (ChannelId, Option<String>) {
    let archive = Archive::open(workspace).ok().flatten();
    resolve_channel_with(archive.as_ref(), workspace, arg)
}

/// Format a channel using its resolved label when available, else its raw ID.
pub(crate) fn display_channel(channel: &ChannelId, label: Option<&str>) -> String {
    label
        .map(str::to_owned)
        .unwrap_or_else(|| channel.to_string())
}
