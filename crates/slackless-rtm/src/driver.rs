//! Background websocket driver: dial, read/write loop, keepalive, and reconnect backoff.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use slackless_types::endpoints::WEB_ORIGIN;
use slackless_types::{SlackEvent, UserId};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};
use tokio::time::{Instant, MissedTickBehavior};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{COOKIE, ORIGIN, USER_AGENT};
use tokio_tungstenite::tungstenite::http::{self, HeaderValue, Request, StatusCode};
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};

use crate::config::{ConnectionState, RtmConfig, RtmEvent};
use crate::frame::{Outgoing, parse_frame};
use crate::url::connect_url_on;

/// Socket error code meaning "socket URL has expired" (drop the stored `reconnect_url`).
const ERROR_URL_EXPIRED: i64 = 1;
/// Socket error code meaning invalid auth (do not reconnect).
const ERROR_INVALID_AUTH: i64 = 401;

pub(crate) enum Command {
    Send(Outgoing),
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Send(o) => f.debug_tuple("Send").field(o).finish(),
        }
    }
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// How a dial attempt or a live session ended.
enum End {
    /// Retry after backoff.
    Lost(String),
    /// Stop for good.
    Stop(String),
}

pub(crate) struct Driver {
    pub(crate) config: RtmConfig,
    pub(crate) tls: Connector,
    pub(crate) commands: mpsc::Receiver<Command>,
    pub(crate) close: watch::Receiver<bool>,
    pub(crate) events: mpsc::Sender<RtmEvent>,
    pub(crate) next_id: u64,
    pub(crate) reconnect_url: Option<String>,
    pub(crate) presence_sub: Option<Vec<UserId>>,
}

async fn wait_for_close(close: &mut watch::Receiver<bool>) {
    if *close.borrow_and_update() {
        return;
    }
    loop {
        if close.changed().await.is_err() || *close.borrow_and_update() {
            return;
        }
    }
}

impl Driver {
    pub(crate) async fn run(mut self) {
        let mut attempt: u32 = 0;
        loop {
            if *self.close.borrow_and_update() {
                self.emit(RtmEvent::State(ConnectionState::Disconnected {
                    reason: "closed by client".into(),
                }))
                .await;
                return;
            }
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
                () = wait_for_close(&mut self.close) => {
                    return Err(End::Stop("closed by client".into()));
                }
                cmd = self.commands.recv() => match cmd {
                    None => return Err(End::Stop("closed by client".into())),
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
        // A server-provided reconnect URL is tried once; a failure or untrusted host falls back to a fresh URL.
        let url = match self.reconnect_url.take() {
            Some(url) if is_trusted_cookie_host(&self.config.base_url, &url) => url,
            Some(url) => {
                let redacted = redact_url_for_log(&url);
                tracing::warn!(
                    url = %redacted,
                    "rejecting untrusted reconnect_url; falling back to base_url"
                );
                connect_url_on(&self.config.base_url, &self.config.params)
                    .map_err(|e| End::Stop(format!("invalid connect url: {e}")))?
                    .into()
            }
            None => connect_url_on(&self.config.base_url, &self.config.params)
                .map_err(|e| End::Stop(format!("invalid connect url: {e}")))?
                .into(),
        };
        let request = self
            .request(&url)
            .map_err(|e| End::Stop(format!("invalid handshake request: {e}")))?;
        let mut dial_fut = Box::pin(tokio::time::timeout(
            Duration::from_secs(20),
            tokio_tungstenite::connect_async_tls_with_config(
                request,
                None,
                false,
                Some(self.tls.clone()),
            ),
        ));
        let result = loop {
            tokio::select! {
                res = &mut dial_fut => match res {
                    Ok(inner) => break inner,
                    Err(_) => return Err(End::Lost("websocket handshake timed out".into())),
                },
                () = wait_for_close(&mut self.close) => {
                    return Err(End::Stop("closed during dial".into()));
                }
                cmd = self.commands.recv() => match cmd {
                    None => return Err(End::Stop("closed during dial".into())),
                    Some(Command::Send(Outgoing::PresenceSub(ids))) => {
                        self.presence_sub = Some(ids);
                    }
                    Some(Command::Send(_)) => {}
                },
            }
        };
        match result {
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

    pub(crate) fn request(&self, url: &str) -> Result<Request<()>, tungstenite::Error> {
        let mut request = url.into_client_request()?;
        let headers = request.headers_mut();
        headers.insert(ORIGIN, HeaderValue::from_static(WEB_ORIGIN));
        if let Some(d) = &self.config.d_cookie
            && is_trusted_cookie_host(&self.config.base_url, url)
        {
            let encoded = encode_cookie(d);
            let value =
                HeaderValue::from_str(&format!("d={encoded}")).map_err(http::Error::from)?;
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
                    let event = match parse_frame(text.as_str()) {
                        Ok(event) => event,
                        Err(err) => {
                            tracing::debug!(error = %err, "ignoring unparseable RTM frame");
                            continue;
                        }
                    };
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
                        SlackEvent::ReconnectUrl(r) => {
                            if is_trusted_cookie_host(&self.config.base_url, &r.url) {
                                self.reconnect_url = Some(r.url.clone());
                            } else {
                                let redacted = redact_url_for_log(&r.url);
                                tracing::warn!(
                                    url = %redacted,
                                    "ignoring untrusted reconnect_url from server"
                                );
                            }
                        }
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
                () = wait_for_close(&mut self.close) => {
                    let close_timeout = self.config.write_timeout.min(Duration::from_secs(2));
                    let _ = tokio::time::timeout(close_timeout, sink.send(Message::Close(None))).await;
                    return End::Stop("closed by client".into());
                }
                cmd = self.commands.recv() => match cmd {
                    None => {
                        let close_timeout = self.config.write_timeout.min(Duration::from_secs(2));
                        let _ = tokio::time::timeout(close_timeout, sink.send(Message::Close(None))).await;
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
            match tokio::time::timeout(
                self.config.write_timeout,
                sink.send(Message::text(outgoing.encode(id))),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(e)) => return End::Lost(e.to_string()),
                Err(_) => return End::Lost("websocket write timed out".into()),
            }
        }
    }

    /// Forwards to the caller; `false` when the receiver is gone.
    async fn emit(&self, event: RtmEvent) -> bool {
        self.events.send(event).await.is_ok()
    }
}

/// URL-encodes the `d` cookie value exactly once. Browsers store it already encoded
/// (`xoxd-...%2F...%3D`); double-encoding breaks auth, so a value made only of URL-safe
/// characters and `%` escapes is kept as is.
pub(crate) fn encode_cookie(value: &str) -> String {
    let already_encoded = value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"-._~%".contains(&b));
    if already_encoded {
        value.to_owned()
    } else {
        url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
    }
}

/// Redact query string, fragment, and userinfo before logging a rejected URL so signed Slack
/// tokens/tickets never land in log files.
pub(crate) fn redact_url_for_log(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(mut parsed) => {
            parsed.set_query(None);
            parsed.set_fragment(None);
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            parsed.to_string()
        }
        Err(_) => "<invalid-url>".to_owned(),
    }
}

/// Returns whether `target_url` is a trusted host for forwarding the `d` session cookie.
///
/// Allows `wss://slack.com` / `wss://*.slack.com`, or a WebSocket URL matching `base_url`'s
/// host without downgrading `wss` to `ws` (preserving local mock WebSocket tests on
/// `127.0.0.1`/`localhost`).
pub(crate) fn is_trusted_cookie_host(base_url: &str, target_url: &str) -> bool {
    let Ok(target) = url::Url::parse(target_url) else {
        return false;
    };
    let Some(target_host) = target.host_str() else {
        return false;
    };
    let target_scheme = target.scheme();
    if !matches!(target_scheme, "ws" | "wss") {
        return false;
    }

    let is_slack = target_scheme == "wss"
        && (target_host == "slack.com" || target_host.ends_with(".slack.com"));
    if is_slack {
        return true;
    }

    let Ok(base) = url::Url::parse(base_url) else {
        return false;
    };
    let Some(base_host) = base.host_str() else {
        return false;
    };
    let scheme_ok = target_scheme == "wss" || base.scheme() == target_scheme;
    scheme_ok && target_host.eq_ignore_ascii_case(base_host)
}
