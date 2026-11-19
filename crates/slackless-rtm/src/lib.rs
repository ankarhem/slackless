//! Realtime websocket client for Slack's web client (`wss-primary`).
//! See docs/research/api-realtime.md.
//!
//! - [`connect_url`] builds the flannel connect URL from a token and [`ConnectParams`].
//! - [`parse_frame`] maps a text frame to [`types::SlackEvent`] (unknown types -> `Unknown`).
//! - [`RtmClient::new`] returns a send handle, an event stream of [`RtmEvent`]s, and a driver
//!   future the caller runs on its own tokio runtime. The driver pings, reconnects with
//!   [`Backoff`], and reports [`ConnectionState`] changes on the stream.

pub use slackless_types as types;

mod backoff;
mod client;
mod frame;
mod url;

pub use backoff::Backoff;
pub use client::{ConnectionState, RtmClient, RtmConfig, RtmError, RtmEvent};
pub use frame::{Outgoing, TickleReason, frame_type, parse_frame};
pub use url::{ConnectParams, DEFAULT_START_ARGS, connect_url, connect_url_on, param};
