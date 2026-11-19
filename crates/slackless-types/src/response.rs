//! The `{ "ok": ..., "error": ..., <payload>, "response_metadata": ... }` envelope every Slack
//! web API method returns.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::de::null_default;

/// A Slack API response with the method-specific payload `T` flattened in.
///
/// `T` only needs the fields it cares about; unknown keys are ignored. Use [`ApiResponse::parse`]
/// to get Slack's `error` string on failure instead of a confusing "missing field" error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    pub ok: bool,
    /// Slack error code (`invalid_auth`, `ratelimited`, `channel_not_found`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    /// Missing scope, on `missing_scope` errors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provided: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_metadata: Option<ResponseMetadata>,
    #[serde(flatten)]
    pub data: T,
}

/// `response_metadata`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseMetadata {
    /// Empty string on the last page.
    #[serde(default)]
    pub next_cursor: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub warnings: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub messages: Vec<String>,
}

/// Payload for methods that return nothing but `ok`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Empty {}

/// The envelope alone, for reading `ok`/`error` before decoding a payload.
pub type ApiEnvelope = ApiResponse<Empty>;

/// Why a response body could not be turned into an `ApiResponse<T>`.
#[derive(Debug, thiserror::Error)]
pub enum ResponseError {
    /// Slack answered `ok: false`.
    #[error("slack error: {}", .0.error.as_deref().unwrap_or("unknown_error"))]
    Slack(Box<ApiEnvelope>),
    /// The body was not JSON, or `ok: true` but the payload did not match `T`.
    #[error("malformed slack response: {0}")]
    Json(#[from] serde_json::Error),
}

impl<T> ApiResponse<T> {
    /// The cursor for the next page, `None` on the last page.
    pub fn next_cursor(&self) -> Option<&str> {
        self.response_metadata
            .as_ref()
            .and_then(|m| m.next_cursor.as_deref())
            .filter(|c| !c.is_empty())
    }
}

impl<T: DeserializeOwned> ApiResponse<T> {
    /// Parses a response body, surfacing `ok: false` as [`ResponseError::Slack`].
    pub fn parse(body: &[u8]) -> Result<Self, ResponseError> {
        Self::from_value(serde_json::from_slice(body)?)
    }

    /// As [`ApiResponse::parse`], from an already-decoded JSON value.
    pub fn from_value(value: Value) -> Result<Self, ResponseError> {
        let envelope = ApiEnvelope::deserialize(&value)?;
        if !envelope.ok {
            return Err(ResponseError::Slack(Box::new(envelope)));
        }
        Ok(serde_json::from_value(value)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Message;

    #[derive(Debug, Deserialize)]
    struct History {
        messages: Vec<Message>,
        #[serde(default)]
        has_more: bool,
    }

    #[test]
    fn parses_paginated_history() {
        let resp = ApiResponse::<History>::parse(
            br#"{"ok": true, "messages": [{"type": "message", "user": "U1", "text": "hi",
                 "ts": "1512085950.000216"}], "has_more": true, "pin_count": 0,
                 "channel_actions_ts": null,
                 "response_metadata": {"next_cursor": "bmV4dF90czoxNTEyMDg1ODYxMDAwNTQz"}}"#,
        )
        .unwrap();
        assert!(resp.data.has_more);
        assert_eq!(resp.data.messages[0].text, "hi");
        assert_eq!(resp.next_cursor(), Some("bmV4dF90czoxNTEyMDg1ODYxMDAwNTQz"));
    }

    #[test]
    fn last_page_has_no_cursor() {
        let resp = ApiResponse::<Empty>::parse(
            br#"{"ok": true, "response_metadata": {"next_cursor": ""}}"#,
        )
        .unwrap();
        assert_eq!(resp.next_cursor(), None);
    }

    #[test]
    fn error_response_surfaces_slack_error() {
        let err = ApiResponse::<History>::parse(br#"{"ok": false, "error": "invalid_auth"}"#)
            .unwrap_err();
        match err {
            ResponseError::Slack(env) => assert_eq!(env.error.as_deref(), Some("invalid_auth")),
            other => panic!("expected Slack error, got {other:?}"),
        }
    }
}
