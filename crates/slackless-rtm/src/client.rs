//! The realtime client: dial, read loop, keepalive, reconnect.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use slackless_types::endpoints::{WEB_ORIGIN, WSS_PRIMARY};
use slackless_types::{ChannelId, SlackEvent, Ts, UserId};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{COOKIE, ORIGIN, USER_AGENT};
use tokio_tungstenite::tungstenite::http::{self, HeaderValue, Request, StatusCode};
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};

use crate::backoff::Backoff;
use crate::frame::{Outgoing, TickleReason, parse_frame};
use crate::url::{ConnectParams, connect_url_on};

/// Socket error code meaning "socket URL has expired" (drop the stored `reconnect_url`).
const ERROR_URL_EXPIRED: i64 = 1;
/// Socket error code meaning invalid auth (do not reconnect).
const ERROR_INVALID_AUTH: i64 = 401;

/// Everything the client needs to connect. Construct with [`RtmConfig::new`].
#[derive(Debug, Clone)]
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
    #[error("tls setup failed: {0}")]
    Tls(#[from] rustls::Error),
}

enum Command {
    Send(Outgoing),
    Close,
}

/// Cheap, cloneable handle for sending frames to the socket.
///
/// Frames sent while disconnected are dropped (typing and pings are stale by then); the last
/// `presence_sub` is re-sent automatically after every `hello`.
#[derive(Debug, Clone)]
pub struct RtmClient {
    commands: mpsc::UnboundedSender<Command>,
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Send(o) => f.debug_tuple("Send").field(o).finish(),
            Self::Close => f.write_str("Close"),
        }
    }
}

impl RtmClient {
    /// Creates the client. Returns the send handle, the event stream, and the driver future.
    ///
    /// Nothing runs until the caller polls the driver on a tokio runtime, e.g.
    /// `tokio::spawn(driver)`. The driver finishes after [`RtmClient::close`], when every
    /// handle is dropped, when the event receiver is dropped, or on a fatal auth error.
    pub fn new(
        config: RtmConfig,
    ) -> Result<
        (
            Self,
            mpsc::Receiver<RtmEvent>,
            impl Future<Output = ()> + Send + 'static,
        ),
        RtmError,
    > {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::channel(config.event_buffer.max(1));
        let driver = Driver {
            tls: tls_connector()?,
            config,
            commands: cmd_rx,
            events: event_tx,
            next_id: 1,
            reconnect_url: None,
            presence_sub: None,
        };
        Ok((Self { commands: cmd_tx }, event_rx, driver.run()))
    }

    /// Queues a frame; it gets the next monotonic `id` when written.
    pub fn send(&self, frame: Outgoing) -> Result<(), RtmError> {
        self.commands
            .send(Command::Send(frame))
            .map_err(|_| RtmError::Closed)
    }

    pub fn user_typing(&self, channel: ChannelId, thread_ts: Option<Ts>) -> Result<(), RtmError> {
        self.send(Outgoing::UserTyping { channel, thread_ts })
    }

    pub fn presence_sub(&self, ids: Vec<UserId>) -> Result<(), RtmError> {
        self.send(Outgoing::PresenceSub(ids))
    }

    pub fn presence_query(&self, ids: Vec<UserId>) -> Result<(), RtmError> {
        self.send(Outgoing::PresenceQuery(ids))
    }

    pub fn tickle(&self, reason: TickleReason) -> Result<(), RtmError> {
        self.send(Outgoing::Tickle(reason))
    }

    /// Closes the socket and stops reconnecting; the stream ends with `Disconnected`.
    pub fn close(&self) {
        let _ = self.commands.send(Command::Close);
    }
}

fn tls_connector() -> Result<Connector, rustls::Error> {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    // Explicit provider: no reliance on a process-wide default crypto provider.
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(Connector::Rustls(Arc::new(config)))
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// How a dial attempt or a live session ended.
enum End {
    /// Retry after backoff.
    Lost(String),
    /// Stop for good.
    Stop(String),
}

struct Driver {
    config: RtmConfig,
    tls: Connector,
    commands: mpsc::UnboundedReceiver<Command>,
    events: mpsc::Sender<RtmEvent>,
    next_id: u64,
    reconnect_url: Option<String>,
    presence_sub: Option<Vec<UserId>>,
}

impl Driver {
    async fn run(mut self) {
        let mut attempt: u32 = 0;
        loop {
            if !self
                .emit(RtmEvent::State(ConnectionState::Connecting))
                .await
            {
                return;
            }
            let started = Instant::now();
            let reason = match self.dial().await {
                Ok(socket) => self.session(socket).await,
                Err(end) => end,
            };
            let reason = match reason {
                End::Lost(reason) => reason,
                End::Stop(reason) => {
                    self.emit(RtmEvent::State(ConnectionState::Disconnected { reason }))
                        .await;
                    return;
                }
            };
            if started.elapsed() >= self.config.healthy_after {
                attempt = 0;
            }
            let delay = self.config.backoff.delay(attempt, &mut rand::rng());
            attempt = attempt.saturating_add(1);
            let state = ConnectionState::Reconnecting {
                attempt,
                delay,
                reason,
            };
            if !self.emit(RtmEvent::State(state)).await {
                return;
            }
            if let Err(End::Stop(reason)) = self.wait(delay).await {
                self.emit(RtmEvent::State(ConnectionState::Disconnected { reason }))
                    .await;
                return;
            }
        }
    }

    /// Sleeps for the backoff delay while still honoring `close`. Sends are dropped.
    async fn wait(&mut self, delay: Duration) -> Result<(), End> {
        let sleep = tokio::time::sleep(delay);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                () = &mut sleep => return Ok(()),
                cmd = self.commands.recv() => match cmd {
                    None | Some(Command::Close) => return Err(End::Stop("closed by client".into())),
                    Some(Command::Send(frame)) => self.remember(&frame),
                },
            }
        }
    }

    fn remember(&mut self, frame: &Outgoing) {
        if let Outgoing::PresenceSub(ids) = frame {
            self.presence_sub = Some(ids.clone());
        }
    }

    async fn dial(&mut self) -> Result<Socket, End> {
        // A server-provided reconnect URL is tried once; a failure falls back to a fresh URL.
        let url = match self.reconnect_url.take() {
            Some(url) => url,
            None => connect_url_on(&self.config.base_url, &self.config.params)
                .map_err(|e| End::Stop(format!("invalid connect url: {e}")))?
                .into(),
        };
        let request = self
            .request(&url)
            .map_err(|e| End::Stop(format!("invalid handshake request: {e}")))?;
        match tokio_tungstenite::connect_async_tls_with_config(
            request,
            None,
            false,
            Some(self.tls.clone()),
        )
        .await
        {
            Ok((socket, _response)) => Ok(socket),
            Err(tungstenite::Error::Http(response))
                if matches!(
                    response.status(),
                    StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
                ) =>
            {
                Err(End::Stop(format!(
                    "handshake rejected: {}",
                    response.status()
                )))
            }
            Err(e @ tungstenite::Error::Url(_)) => Err(End::Stop(e.to_string())),
            Err(e) => Err(End::Lost(e.to_string())),
        }
    }

    fn request(&self, url: &str) -> Result<Request<()>, tungstenite::Error> {
        let mut request = url.into_client_request()?;
        let headers = request.headers_mut();
        headers.insert(ORIGIN, HeaderValue::from_static(WEB_ORIGIN));
        if let Some(d) = &self.config.d_cookie {
            let value = HeaderValue::from_str(&format!("d={d}")).map_err(http::Error::from)?;
            headers.insert(COOKIE, value);
        }
        if let Some(ua) = &self.config.user_agent {
            headers.insert(
                USER_AGENT,
                HeaderValue::from_str(ua).map_err(http::Error::from)?,
            );
        }
        Ok(request)
    }

    async fn session(&mut self, socket: Socket) -> End {
        let (mut sink, mut stream) = socket.split();
        let period = self.config.ping_interval;
        let mut ping = tokio::time::interval_at(Instant::now() + period, period);
        ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut unanswered: u32 = 0;

        loop {
            let outgoing = tokio::select! {
                frame = stream.next() => {
                    let text = match frame {
                        None => return End::Lost("socket closed".into()),
                        Some(Err(e)) => return End::Lost(e.to_string()),
                        Some(Ok(Message::Text(text))) => text,
                        Some(Ok(Message::Close(close))) => {
                            return End::Lost(match close {
                                Some(c) => format!("closed by server: {} {}", c.code, c.reason),
                                None => "closed by server".into(),
                            });
                        }
                        // Websocket-level pings are answered by tungstenite itself while reading.
                        Some(Ok(_)) => continue,
                    };
                    let Ok(event) = parse_frame(text.as_str()) else { continue };
                    let mut end = None;
                    let mut resubscribe = None;
                    match &event {
                        SlackEvent::Hello(hello) => {
                            let state = ConnectionState::Connected { fast_reconnect: hello.fast_reconnect };
                            if !self.emit(RtmEvent::State(state)).await {
                                return End::Stop("event receiver dropped".into());
                            }
                            resubscribe = self.presence_sub.clone().map(Outgoing::PresenceSub);
                        }
                        SlackEvent::Pong(_) => unanswered = 0,
                        SlackEvent::ReconnectUrl(r) => self.reconnect_url = Some(r.url.clone()),
                        SlackEvent::Goodbye(_) => end = Some(End::Lost("server said goodbye".into())),
                        SlackEvent::Error(e) => match e.error.code {
                            Some(ERROR_URL_EXPIRED) => self.reconnect_url = None,
                            Some(ERROR_INVALID_AUTH) => {
                                end = Some(End::Stop(format!("invalid auth: {}", e.error.msg.as_deref().unwrap_or_default())));
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                    if !self.emit(RtmEvent::Event(event)).await {
                        return End::Stop("event receiver dropped".into());
                    }
                    if let Some(end) = end {
                        return end;
                    }
                    match resubscribe {
                        Some(frame) => frame,
                        None => continue,
                    }
                }
                _ = ping.tick() => {
                    if unanswered >= self.config.max_missed_pongs {
                        return End::Lost(format!("{unanswered} pings unanswered"));
                    }
                    unanswered += 1;
                    Outgoing::Ping
                }
                cmd = self.commands.recv() => match cmd {
                    None | Some(Command::Close) => {
                        let _ = sink.send(Message::Close(None)).await;
                        return End::Stop("closed by client".into());
                    }
                    Some(Command::Send(frame)) => {
                        self.remember(&frame);
                        frame
                    }
                },
            };
            let id = self.next_id;
            self.next_id += 1;
            if let Err(e) = sink.send(Message::text(outgoing.encode(id))).await {
                return End::Lost(e.to_string());
            }
        }
    }

    /// Forwards to the caller; `false` when the receiver is gone.
    async fn emit(&self, event: RtmEvent) -> bool {
        self.events.send(event).await.is_ok()
    }
}
