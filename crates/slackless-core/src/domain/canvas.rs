//! Shared Canvas domain logic: comment filtering, share target parsing, general-access summaries,
//! and API error formatting.

use crate::api::{
    ApiError, CanvasAccessLevel, CanvasCommentThread, CanvasMeta, format_canvas_error,
};
use crate::types::{ChannelId, UserId};

/// Filter tab for Canvas comment threads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CanvasCommentFilter {
    #[default]
    All,
    Open,
    Resolved,
}

/// Filter comment threads by [`CanvasCommentFilter`].
pub fn filter_comment_threads(
    threads: &[CanvasCommentThread],
    filter: CanvasCommentFilter,
) -> Vec<&CanvasCommentThread> {
    threads
        .iter()
        .filter(|t| match filter {
            CanvasCommentFilter::All => true,
            CanvasCommentFilter::Open => !t.resolved,
            CanvasCommentFilter::Resolved => t.resolved,
        })
        .collect()
}

/// Compute `(total, open, resolved)` counts for Canvas comment threads.
pub fn comment_thread_counts(threads: &[CanvasCommentThread]) -> (usize, usize, usize) {
    let total = threads.len();
    let resolved = threads.iter().filter(|t| t.resolved).count();
    let open = total.saturating_sub(resolved);
    (total, open, resolved)
}

/// Validated share/access-grant request for a Canvas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareAccessRequest {
    pub access_level: CanvasAccessLevel,
    pub channel_ids: Vec<ChannelId>,
    pub user_ids: Vec<UserId>,
    /// `Some(permalink)` posts the canvas link to shared channels; `None` grants silent access.
    pub post_permalink: Option<String>,
}

/// Parse comma-, semicolon-, or whitespace-separated channel/user IDs (e.g. `#C01ENG, @U01ALICE`).
pub fn parse_share_targets(raw: &str) -> (Vec<ChannelId>, Vec<UserId>) {
    let mut channels = Vec::new();
    let mut users = Vec::new();
    for token in raw.split(|c: char| c == ',' || c == ';' || c.is_whitespace()) {
        let clean = token.trim().trim_start_matches('#').trim_start_matches('@');
        if clean.len() < 3 {
            continue;
        }
        let upper = clean.to_ascii_uppercase();
        if !upper
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        {
            continue;
        }
        match upper.as_bytes()[0] {
            b'C' | b'G' | b'D' => {
                let cid = ChannelId::from(upper.as_str());
                if !channels.contains(&cid) {
                    channels.push(cid);
                }
            }
            b'U' | b'W' => {
                let uid = UserId::from(upper.as_str());
                if !users.contains(&uid) {
                    users.push(uid);
                }
            }
            _ => {}
        }
    }
    (channels, users)
}

/// Validate and build a [`ShareAccessRequest`] from raw target IDs and share options.
pub fn build_share_access_request(
    raw_targets: &str,
    access_level: CanvasAccessLevel,
    share_without_posting: bool,
    permalink: Option<&str>,
) -> Result<ShareAccessRequest, String> {
    let (channel_ids, user_ids) = parse_share_targets(raw_targets);
    if channel_ids.is_empty() && user_ids.is_empty() {
        return Err(
            "Enter at least one Slack channel ID (C…) or user ID (U…) to share with.".to_owned(),
        );
    }
    let post_permalink = if share_without_posting || channel_ids.is_empty() {
        None
    } else {
        permalink
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    Ok(ShareAccessRequest {
        access_level,
        channel_ids,
        user_ids,
        post_permalink,
    })
}

/// Summarize a canvas's general access badge and description.
pub fn general_access_summary(
    meta: Option<&CanvasMeta>,
    local_override: Option<&str>,
    limit_sharing: bool,
) -> (&'static str, String) {
    let raw = local_override
        .filter(|s| !s.is_empty())
        .or_else(|| meta.and_then(|m| m.org_or_workspace_access.as_deref()))
        .or_else(|| meta.and_then(|m| m.access.as_deref()))
        .unwrap_or("private")
        .to_ascii_lowercase();

    let (badge, mut desc) = match raw.as_str() {
        "write" | "edit" | "workspace_edit" => (
            "Workspace can edit",
            "Anyone in this workspace with the link can view, comment, and edit.".to_owned(),
        ),
        "read" | "view" | "workspace_view" => (
            "Workspace can view",
            "Anyone in this workspace with the link can view and comment.".to_owned(),
        ),
        _ => (
            "Invite only",
            "Only people and channels explicitly added below can open this canvas.".to_owned(),
        ),
    };
    if limit_sharing {
        desc.push_str(" Re-sharing is limited to canvas owners.");
    }
    (badge, desc)
}

/// Translate a Slack [`ApiError`] from a canvas endpoint into clear user-facing guidance.
pub fn format_canvas_api_error(err: &ApiError) -> String {
    match err {
        ApiError::Slack(code) | ApiError::AuthExpired(code) => format_canvas_error(code),
        other => format_canvas_error(&other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Ts;

    #[test]
    fn comment_filters_and_counts_work() {
        let threads = vec![
            CanvasCommentThread {
                root_ts: Ts::from("1.0"),
                author: None,
                text: "open".into(),
                reply_count: 0,
                replies: vec![],
                resolved: false,
            },
            CanvasCommentThread {
                root_ts: Ts::from("2.0"),
                author: None,
                text: "done".into(),
                reply_count: 0,
                replies: vec![],
                resolved: true,
            },
        ];
        assert_eq!(comment_thread_counts(&threads), (2, 1, 1));
        assert_eq!(
            filter_comment_threads(&threads, CanvasCommentFilter::All).len(),
            2
        );
        assert_eq!(
            filter_comment_threads(&threads, CanvasCommentFilter::Open).len(),
            1
        );
        assert_eq!(
            filter_comment_threads(&threads, CanvasCommentFilter::Resolved).len(),
            1
        );
    }

    #[test]
    fn parse_share_targets_deduplicates_normalizes_and_ignores_invalid() {
        let (channels, users) =
            parse_share_targets("#c01eng, C01ENG; @u01alice W02BOB G03PRIV D04DM bad X99999");
        assert_eq!(
            channels,
            vec![
                ChannelId::from("C01ENG"),
                ChannelId::from("G03PRIV"),
                ChannelId::from("D04DM"),
            ]
        );
        assert_eq!(
            users,
            vec![UserId::from("U01ALICE"), UserId::from("W02BOB")]
        );

        let err = build_share_access_request("???", CanvasAccessLevel::Read, false, None);
        assert!(err.is_err());

        let req = build_share_access_request(
            "C01ENG",
            CanvasAccessLevel::Write,
            false,
            Some("https://acme.slack.com/docs/T1/F1"),
        )
        .expect("valid request");
        assert_eq!(
            req.post_permalink.as_deref(),
            Some("https://acme.slack.com/docs/T1/F1")
        );
    }

    #[test]
    fn format_canvas_api_error_translates_slack_codes() {
        let msg = format_canvas_api_error(&ApiError::Slack("restricted_action".into()));
        assert!(msg.contains("permission"));
    }
}
