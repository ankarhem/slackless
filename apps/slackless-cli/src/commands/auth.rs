use std::process::ExitCode;

use anyhow::{Context, Result};
use slackless_core::auth::SessionStore;

use crate::{Ctx, output, session};

pub(crate) fn cmd_auth_status(ctx: &Ctx) -> Result<ExitCode> {
    let entries = session::status_entries();
    let view = output::AuthStatusView {
        signed_in: entries.iter().any(|entry| entry.has_secrets),
        workspaces: entries
            .iter()
            .map(output::WorkspaceView::from_entry)
            .collect(),
    };
    output::emit(ctx.json, &view, &output::auth_human(&view));
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn cmd_auth_login(ctx: &Ctx, workspace_domain: Option<String>) -> Result<ExitCode> {
    let sessions =
        slackless_auth::embedded_sign_in(slackless_auth::SignInRequest { workspace_domain })
            .context("signing in")?;
    let store = SessionStore::new()?;
    for session in sessions {
        store.upsert(session)?;
    }
    cmd_auth_status(ctx)
}

pub(crate) fn cmd_auth_logout(ctx: &Ctx, workspace: Option<&str>) -> Result<ExitCode> {
    let store = SessionStore::new()?;
    for session in store.load()? {
        if workspace.is_none_or(|team| team == session.team_id.as_str()) {
            store.remove(&session.team_id)?;
        }
    }
    cmd_auth_status(ctx)
}
