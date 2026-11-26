//! Durable offline send outbox payloads and transient-error classification shared between
//! `slackless-core` and front-ends.

use crate::api::{ApiError, MessageContent};
use crate::store::{OutboxItem, OutboxKind};
use crate::types::Ts;

/// Serialized payload stored in `outbox.payload_json`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutboxPayload {
    Message {
        content: MessageContent,
    },
    Reply {
        content: MessageContent,
        #[serde(default)]
        broadcast: bool,
    },
    ReactionAdd {
        ts: Ts,
        name: String,
    },
    ReactionRemove {
        ts: Ts,
        name: String,
    },
}

impl OutboxPayload {
    /// Corresponding SQLite `outbox.kind` discriminator.
    pub const fn kind(&self) -> OutboxKind {
        match self {
            Self::Message { .. } => OutboxKind::Message,
            Self::Reply { .. } => OutboxKind::Reply,
            Self::ReactionAdd { .. } => OutboxKind::ReactionAdd,
            Self::ReactionRemove { .. } => OutboxKind::ReactionRemove,
        }
    }

    /// Encode this payload to JSON for SQLite storage.
    pub fn encode(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Decode a persisted `outbox.payload_json` string.
    pub fn decode(raw: &str) -> Option<Self> {
        serde_json::from_str(raw).ok()
    }

    /// The message body content when this entry is a channel message or thread reply.
    pub fn content(&self) -> Option<&MessageContent> {
        match self {
            Self::Message { content } | Self::Reply { content, .. } => Some(content),
            Self::ReactionAdd { .. } | Self::ReactionRemove { .. } => None,
        }
    }

    /// Human-readable single-line preview for inline cards and top-bar menus.
    pub fn preview_text(&self) -> String {
        match self {
            Self::Message { content } | Self::Reply { content, .. } => {
                let text = content.text.trim();
                if text.is_empty() {
                    content
                        .attachments
                        .first()
                        .and_then(|a| a.fallback.as_deref())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .unwrap_or("Message")
                        .to_owned()
                } else {
                    text.to_owned()
                }
            }
            Self::ReactionAdd { name, .. } => format!("Add reaction :{name}:"),
            Self::ReactionRemove { name, .. } => format!("Remove reaction :{name}:"),
        }
    }

    /// Compact summary label (truncated to 48 chars) for menus and logs.
    pub fn summary(&self) -> String {
        let raw = self.preview_text();
        let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        let preview: String = collapsed.chars().take(48).collect();
        if collapsed.chars().count() > 48 {
            format!("{preview}…")
        } else {
            preview
        }
    }
}

/// Decode the [`OutboxPayload`] carried by an [`OutboxItem`].
pub fn decode_outbox_payload(item: &OutboxItem) -> Option<OutboxPayload> {
    OutboxPayload::decode(&item.payload_json)
}

/// Returns `true` when `err` represents an offline, network, or transient transport failure that
/// should be queued in the durable outbox rather than surfaced as a permanent rejection.
///
/// Permanent Slack API errors (`channel_not_found`, `not_in_channel`, `invalid_auth`,
/// `token_revoked`, `is_archived`, `msg_too_long`, `already_reacted`, `no_reaction`, etc.)
/// strictly return `false`.
pub fn is_transient_send_error(err: &ApiError) -> bool {
    match err {
        ApiError::Http(_) | ApiError::RateLimited { .. } => true,
        ApiError::Status { status, .. } => *status == 408 || *status == 429 || *status >= 500,
        ApiError::Slack(code) => matches!(
            code.as_str(),
            "ratelimited"
                | "internal_error"
                | "service_unavailable"
                | "request_timeout"
                | "fatal_error"
        ),
        ApiError::AuthExpired(_)
        | ApiError::FileAuthRejected
        | ApiError::UntrustedUrl(_)
        | ApiError::Url(_)
        | ApiError::Decode(_)
        | ApiError::UnsupportedHuddleBackend(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn transient_error_classification_rejects_permanent_slack_and_auth_errors() {
        assert!(is_transient_send_error(&ApiError::RateLimited {
            retry_after: Duration::from_secs(2),
        }));
        assert!(is_transient_send_error(&ApiError::Status {
            status: 503,
            body: "unavailable".into(),
        }));
        assert!(is_transient_send_error(&ApiError::Status {
            status: 408,
            body: "timeout".into(),
        }));
        assert!(is_transient_send_error(&ApiError::Status {
            status: 429,
            body: "too many".into(),
        }));
        assert!(is_transient_send_error(&ApiError::Slack(
            "service_unavailable".into()
        )));
        assert!(is_transient_send_error(&ApiError::Slack(
            "internal_error".into()
        )));

        // Permanent errors must NOT be queued in the outbox.
        assert!(!is_transient_send_error(&ApiError::Slack(
            "channel_not_found".into()
        )));
        assert!(!is_transient_send_error(&ApiError::Slack(
            "not_in_channel".into()
        )));
        assert!(!is_transient_send_error(&ApiError::Slack(
            "is_archived".into()
        )));
        assert!(!is_transient_send_error(&ApiError::AuthExpired(
            "invalid_auth".into()
        )));
        assert!(!is_transient_send_error(&ApiError::Status {
            status: 400,
            body: "bad request".into(),
        }));
        assert!(!is_transient_send_error(&ApiError::Status {
            status: 403,
            body: "forbidden".into(),
        }));
    }

    #[test]
    fn outbox_payload_round_trips_and_truncates_utf8_cleanly() {
        let long = "🎉".repeat(60);
        let p = OutboxPayload::Message {
            content: MessageContent::plain(long),
        };
        let encoded = p.encode();
        let decoded = OutboxPayload::decode(&encoded).expect("decode message");
        assert_eq!(decoded, p);
        assert_eq!(decoded.kind(), OutboxKind::Message);
        let summary = decoded.summary();
        assert!(summary.ends_with('…'));
        assert_eq!(summary.chars().filter(|&c| c == '🎉').count(), 48);

        let rx = OutboxPayload::ReactionAdd {
            ts: Ts::from("1700000001.000100"),
            name: "rocket".into(),
        };
        assert_eq!(rx.preview_text(), "Add reaction :rocket:");
        assert_eq!(OutboxPayload::decode(&rx.encode()), Some(rx));
    }
}
