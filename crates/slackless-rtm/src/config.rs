//! Configuration, connection lifecycle states, events, and errors for the RTM client.

use std::fmt;
use std::time::Duration;

use slackless_types::SlackEvent;
use slackless_types::endpoints::WSS_PRIMARY;

use crate::backoff::Backoff;
use crate::url::ConnectParams;

/// Default capacity of the bounded outgoing command channel.
pub(crate) const DEFAULT_COMMAND_BUFFER: usize = 256;
/// Default timeout for writing a single websocket frame.
pub(crate) const DEFAULT_WRITE_TIMEOUT: Duration = Duration::from_secs(15);

/// Everything the client needs to connect. Construct with [`RtmConfig::new`].
#[derive(Clone)]
pub struct RtmConfig {
    pub params: ConnectParams,
    /// Websocket host; defaults to [`WSS_PRIMARY`].
    pub base_url: String,
    /// Value of the `d` session cookie (`xoxd-...`), sent as `Cookie: d=<value>`.
    pub d_cookie: Option<String>,
    pub user_agent: Option<String>,
    /// Interval between app-level `ping` frames.
    pub ping_interval: Duration,
    /// Reconnect after this many consecutive pings go unanswered.
    pub max_missed_pongs: u32,
    pub backoff: Backoff,
    /// A session that lived this long resets the backoff to its first step.
    pub healthy_after: Duration,
    /// Capacity of the event channel (backpressure: the read loop waits when it is full).
    pub event_buffer: usize,
    /// Capacity of the bounded outgoing command channel.
    pub command_buffer: usize,
    /// Timeout applied to websocket frame writes before treating the connection as stalled.
    pub write_timeout: Duration,
}

impl fmt::Debug for RtmConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RtmConfig")
            .field("params", &self.params)
            .field("base_url", &self.base_url)
            .field("d_cookie", &self.d_cookie.as_ref().map(|_| "<redacted>"))
            .field("user_agent", &self.user_agent)
            .field("ping_interval", &self.ping_interval)
            .field("max_missed_pongs", &self.max_missed_pongs)
            .field("backoff", &self.backoff)
            .field("healthy_after", &self.healthy_after)
            .field("event_buffer", &self.event_buffer)
            .field("command_buffer", &self.command_buffer)
            .field("write_timeout", &self.write_timeout)
            .finish()
    }
}

impl RtmConfig {
    pub fn new(params: ConnectParams) -> Self {
        Self {
            params,
            base_url: WSS_PRIMARY.to_owned(),
            d_cookie: None,
            user_agent: None,
            ping_interval: Duration::from_secs(15),
            max_missed_pongs: 3,
            backoff: Backoff::default(),
            healthy_after: Duration::from_secs(60),
            event_buffer: 256,
            command_buffer: DEFAULT_COMMAND_BUFFER,
            write_timeout: DEFAULT_WRITE_TIMEOUT,
        }
    }
}

/// Connection lifecycle, emitted on the event stream as [`RtmEvent::State`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    /// Dialing the socket.
    Connecting,
    /// `hello` received; the socket is live.
    Connected { fast_reconnect: bool },
    /// The connection dropped; the next dial happens after `delay`.
    Reconnecting {
        attempt: u32,
        delay: Duration,
        reason: String,
    },
    /// Terminal: closed by the caller or a non-retryable error. The stream ends after this.
    Disconnected { reason: String },
}

/// One item on the event stream.
#[derive(Debug, Clone, PartialEq)]
pub enum RtmEvent {
    State(ConnectionState),
    /// Every parsed frame, control frames (`hello`, `pong`, `goodbye`, ...) included.
    Event(SlackEvent),
}

#[derive(Debug, thiserror::Error)]
pub enum RtmError {
    #[error("rtm client is shut down")]
    Closed,
    #[error("rtm outgoing command queue is full")]
    QueueFull,
    #[error("tls setup failed: {0}")]
    Tls(#[from] rustls::Error),
}
