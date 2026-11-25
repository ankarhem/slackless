//! `status` (`show`, `set`, `clear`) live commands and status formatting/normalization helpers.

use std::process::ExitCode;

use anyhow::Result;
use slackless_core::api::ProfileUpdate;
use slackless_core::types::time::now_epoch_secs;
use slackless_core::types::{Profile, UserId};

use crate::Ctx;
use crate::output;

use super::session::{api_fail, api_for};

fn format_status_pair(emoji: &str, text: &str) -> String {
    match (emoji.is_empty(), text.is_empty()) {
        (true, true) => String::new(),
        (true, false) => text.to_owned(),
        (false, true) => emoji.to_owned(),
        (false, false) => format!("{emoji} {text}"),
    }
}

pub(crate) fn format_status_show(
    user_id: &UserId,
    profile: &Profile,
) -> (serde_json::Value, String) {
    let text = profile.status_text.as_deref().unwrap_or_default();
    let emoji = profile.status_emoji.as_deref().unwrap_or_default();
    let expiration = profile.status_expiration.unwrap_or(0);
    let rendered = format_status_pair(emoji, text);
    let human = if rendered.is_empty() {
        "(no status set)\n".to_owned()
    } else {
        format!("{rendered}\n")
    };
    let json = serde_json::json!({
        "ok": true,
        "user": user_id.as_str(),
        "status_text": text,
        "status_emoji": emoji,
        "status_expiration": expiration,
    });
    (json, human)
}

/// `status show`
pub async fn run_status_show(ctx: &Ctx, flag: Option<&str>) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    match session.api.users_profile_get(&session.user_id).await {
        Ok(profile) => {
            let (json, human) = format_status_show(&session.user_id, &profile);
            output::emit(ctx.json, &json, &human);
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

/// Normalize a status emoji argument into `:shortcode:` (or empty string to clear).
pub(crate) fn normalize_status_emoji(emoji: Option<&str>) -> Option<String> {
    emoji.map(|e| {
        let trimmed = e.trim().trim_matches(':');
        if trimmed.is_empty() {
            String::new()
        } else {
            format!(":{trimmed}:")
        }
    })
}

/// Convert `--expires-in SECS` into a unix epoch expiration (`0` for non-positive values).
pub(crate) fn compute_status_expiration(expires_in: Option<i64>, now_secs: i64) -> Option<i64> {
    expires_in.map(|secs| {
        if secs <= 0 {
            0
        } else {
            now_secs.saturating_add(secs)
        }
    })
}

pub(crate) fn format_status_set(
    requested_text: String,
    profile: Profile,
) -> (serde_json::Value, String) {
    let em = profile.status_emoji.unwrap_or_default();
    let txt = profile.status_text.unwrap_or(requested_text);
    let exp = profile.status_expiration.unwrap_or(0);
    let json = serde_json::json!({
        "ok": true,
        "status_text": txt,
        "status_emoji": em,
        "status_expiration": exp,
    });
    let rendered = format_status_pair(&em, &txt);
    let human = if rendered.is_empty() {
        "status set: (empty)\n".to_owned()
    } else {
        format!("status set: {rendered}\n")
    };
    (json, human)
}

/// `status set <TEXT> [--emoji EMOJI] [--expires-in SECS]`
pub async fn run_status_set(
    ctx: &Ctx,
    flag: Option<&str>,
    text: String,
    emoji: Option<String>,
    expires_in: Option<i64>,
) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let status_emoji = normalize_status_emoji(emoji.as_deref());
    let status_expiration = compute_status_expiration(expires_in, now_epoch_secs());
    let update = ProfileUpdate {
        status_text: Some(text.clone()),
        status_emoji,
        status_expiration,
        ..Default::default()
    };
    match session.api.users_profile_set(&update).await {
        Ok(profile) => {
            let (json, human) = format_status_set(text, profile);
            output::emit(ctx.json, &json, &human);
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}

/// `status clear`
pub async fn run_status_clear(ctx: &Ctx, flag: Option<&str>) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let update = ProfileUpdate {
        status_text: Some(String::new()),
        status_emoji: Some(String::new()),
        status_expiration: Some(0),
        ..Default::default()
    };
    match session.api.users_profile_set(&update).await {
        Ok(_) => {
            output::emit(
                ctx.json,
                &serde_json::json!({ "ok": true, "cleared": true }),
                "status cleared\n",
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => Ok(api_fail(ctx, &err)),
    }
}
