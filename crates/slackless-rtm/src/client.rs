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
                cmd = self.commands.recv() => match cmd {
                    Some(Command::Close) | None => return Err(End::Stop("closed during dial".into())),
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn next_event(rx: &mut mpsc::Receiver<RtmEvent>) -> RtmEvent {
        tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .expect("timed out waiting for RTM event")
            .expect("RTM event stream ended unexpectedly")
    }

    #[tokio::test]
    async fn hello_replays_presence_sub_ignores_bad_frames_and_reconnects_on_goodbye() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            // Session 1: accept, send hello + garbage frame, expect presence_sub, send goodbye.
            let (stream1, _) = listener.accept().await.unwrap();
            let mut ws1 = tokio_tungstenite::accept_async(stream1).await.unwrap();
            ws1.send(Message::text(r#"{"type":"hello"}"#))
                .await
                .unwrap();
            ws1.send(Message::text("not valid json {{{")).await.unwrap();
            let sub1 = ws1.next().await.unwrap().unwrap().into_text().unwrap();
            assert!(sub1.contains("presence_sub"), "got: {sub1}");
            assert!(sub1.contains("U123"), "got: {sub1}");
            ws1.send(Message::text(r#"{"type":"goodbye"}"#))
                .await
                .unwrap();
            let _ = ws1.close(None).await;

            // Session 2: accept reconnect, send hello, verify presence_sub is automatically replayed.
            let (stream2, _) = listener.accept().await.unwrap();
            let mut ws2 = tokio_tungstenite::accept_async(stream2).await.unwrap();
            ws2.send(Message::text(r#"{"type":"hello","fast_reconnect":true}"#))
                .await
                .unwrap();
            let sub2 = ws2.next().await.unwrap().unwrap().into_text().unwrap();
            assert!(sub2.contains("presence_sub"), "got: {sub2}");
            assert!(sub2.contains("U123"), "got: {sub2}");
            // Keep open briefly until client closes.
            let _ = ws2.next().await;
        });

        let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-test"));
        cfg.base_url = format!("ws://{addr}/link/");
        cfg.backoff = Backoff {
            initial: Duration::from_millis(10),
            max: Duration::from_millis(50),
        };
        let (client, mut rx, driver) = RtmClient::new(cfg).unwrap();
        let drv = tokio::spawn(driver);

        // Queue presence_sub before hello arrives; driver must replay it after hello.
        client.presence_sub(vec![UserId::from("U123")]).unwrap();

        assert_eq!(
            next_event(&mut rx).await,
            RtmEvent::State(ConnectionState::Connecting)
        );
        assert_eq!(
            next_event(&mut rx).await,
            RtmEvent::State(ConnectionState::Connected {
                fast_reconnect: false
            })
        );
        // Hello event + Goodbye event + Reconnecting + Connecting + Connected (fast_reconnect: true)
        let mut saw_reconnect = false;
        let mut saw_second_connect = false;
        for _ in 0..8 {
            match next_event(&mut rx).await {
                RtmEvent::State(ConnectionState::Reconnecting { .. }) => saw_reconnect = true,
                RtmEvent::State(ConnectionState::Connected {
                    fast_reconnect: true,
                }) => {
                    saw_second_connect = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(saw_reconnect);
        assert!(saw_second_connect);

        client.close();
        let _ = tokio::time::timeout(Duration::from_secs(2), drv).await;
        server.await.unwrap();
    }

    #[tokio::test]
    async fn unauthorized_handshake_stops_without_reconnecting() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf).await;
            let _ = sock
                .write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n")
                .await;
            let _ = sock.shutdown().await;
        });

        let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-expired"));
        cfg.base_url = format!("ws://{addr}/link/");
        let (_client, mut rx, driver) = RtmClient::new(cfg).unwrap();
        let drv = tokio::spawn(driver);

        assert_eq!(
            next_event(&mut rx).await,
            RtmEvent::State(ConnectionState::Connecting)
        );
        match next_event(&mut rx).await {
            RtmEvent::State(ConnectionState::Disconnected { reason }) => {
                assert!(reason.contains("401"), "got: {reason}");
            }
            other => panic!("expected Disconnected on 401, got {other:?}"),
        }
        tokio::time::timeout(Duration::from_secs(2), drv)
            .await
            .expect("driver should exit promptly on 401")
            .unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn ping_pong_liveness_resets_on_pong_and_reconnects_after_max_missed_pongs() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            // Session 1: answer 1st ping with pong (resets unanswered counter), ignore subsequent pings.
            let (stream1, _) = listener.accept().await.unwrap();
            let mut ws1 = tokio_tungstenite::accept_async(stream1).await.unwrap();
            ws1.send(Message::text(r#"{"type":"hello"}"#))
                .await
                .unwrap();

            let ping1: serde_json::Value =
                serde_json::from_str(&ws1.next().await.unwrap().unwrap().into_text().unwrap())
                    .unwrap();
            assert_eq!(ping1["type"], "ping");
            let id1 = ping1["id"].as_u64().unwrap();
            ws1.send(Message::text(format!(
                r#"{{"type":"pong","reply_to":{id1}}}"#
            )))
            .await
            .unwrap();

            // Next 2 pings go unanswered (max_missed_pongs = 2); on the 3rd tick client drops.
            let ping2: serde_json::Value =
                serde_json::from_str(&ws1.next().await.unwrap().unwrap().into_text().unwrap())
                    .unwrap();
            assert_eq!(ping2["type"], "ping");
            let ping3: serde_json::Value =
                serde_json::from_str(&ws1.next().await.unwrap().unwrap().into_text().unwrap())
                    .unwrap();
            assert_eq!(ping3["type"], "ping");

            // Session 2 confirms the client reconnected after missing pongs.
            let (stream2, _) = listener.accept().await.unwrap();
            let mut ws2 = tokio_tungstenite::accept_async(stream2).await.unwrap();
            ws2.send(Message::text(r#"{"type":"hello"}"#))
                .await
                .unwrap();
            let _ = ws2.next().await;
        });

        let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-ping"));
        cfg.base_url = format!("ws://{addr}/link/");
        cfg.ping_interval = Duration::from_millis(25);
        cfg.max_missed_pongs = 2;
        cfg.backoff = Backoff {
            initial: Duration::from_millis(10),
            max: Duration::from_millis(20),
        };
        let (client, mut rx, driver) = RtmClient::new(cfg).unwrap();
        let drv = tokio::spawn(driver);

        let mut saw_pong = false;
        let mut saw_missed_reconnect = false;
        let mut connected_count = 0;
        for _ in 0..15 {
            match next_event(&mut rx).await {
                RtmEvent::Event(SlackEvent::Pong(_)) => saw_pong = true,
                RtmEvent::State(ConnectionState::Reconnecting { reason, .. }) => {
                    assert!(reason.contains("2 pings unanswered"), "got: {reason}");
                    saw_missed_reconnect = true;
                }
                RtmEvent::State(ConnectionState::Connected { .. }) => {
                    connected_count += 1;
                    if connected_count == 2 {
                        break;
                    }
                }
                _ => {}
            }
        }
        assert!(saw_pong);
        assert!(saw_missed_reconnect);
        assert_eq!(connected_count, 2);

        client.close();
        let _ = tokio::time::timeout(Duration::from_secs(2), drv).await;
        server.await.unwrap();
    }

    #[tokio::test]
    async fn reconnect_url_is_single_use_and_falls_back_to_base_url() {
        let base_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_addr = base_listener.local_addr().unwrap();
        let redir_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let redir_addr = redir_listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            // Session 1 on base_url: send reconnect_url pointing to redir_listener, then goodbye.
            let (s1, _) = base_listener.accept().await.unwrap();
            let mut ws1 = tokio_tungstenite::accept_async(s1).await.unwrap();
            ws1.send(Message::text(r#"{"type":"hello"}"#))
                .await
                .unwrap();
            ws1.send(Message::text(format!(
                r#"{{"type":"reconnect_url","url":"ws://{redir_addr}/custom-path"}}"#
            )))
            .await
            .unwrap();
            ws1.send(Message::text(r#"{"type":"goodbye"}"#))
                .await
                .unwrap();
            let _ = ws1.close(None).await;

            // Session 2 MUST dial redir_listener with `/custom-path`.
            let (s2, _) = redir_listener.accept().await.unwrap();
            let mut seen_path = String::new();
            #[allow(clippy::result_large_err)]
            let mut ws2 = tokio_tungstenite::accept_hdr_async(
                s2,
                |req: &tungstenite::handshake::server::Request,
                 resp: tungstenite::handshake::server::Response| {
                    seen_path = req.uri().path().to_owned();
                    Ok(resp)
                },
            )
            .await
            .unwrap();
            assert_eq!(seen_path, "/custom-path");
            ws2.send(Message::text(r#"{"type":"hello","fast_reconnect":true}"#))
                .await
                .unwrap();
            ws2.send(Message::text(r#"{"type":"goodbye"}"#))
                .await
                .unwrap();
            let _ = ws2.close(None).await;

            // Session 3 MUST fall back to base_listener (reconnect_url was single-use).
            let (s3, _) = base_listener.accept().await.unwrap();
            let mut ws3 = tokio_tungstenite::accept_async(s3).await.unwrap();
            ws3.send(Message::text(r#"{"type":"hello"}"#))
                .await
                .unwrap();
            let _ = ws3.next().await;
        });

        let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-redir"));
        cfg.base_url = format!("ws://{base_addr}/link/");
        cfg.backoff = Backoff {
            initial: Duration::from_millis(10),
            max: Duration::from_millis(20),
        };
        let (client, mut rx, driver) = RtmClient::new(cfg).unwrap();
        let drv = tokio::spawn(driver);

        let mut connects = Vec::new();
        for _ in 0..20 {
            if let RtmEvent::State(ConnectionState::Connected { fast_reconnect }) =
                next_event(&mut rx).await
            {
                connects.push(fast_reconnect);
                if connects.len() == 3 {
                    break;
                }
            }
        }
        assert_eq!(connects, vec![false, true, false]);

        client.close();
        let _ = tokio::time::timeout(Duration::from_secs(2), drv).await;
        server.await.unwrap();
    }

    #[tokio::test]
    async fn error_url_expired_clears_stored_reconnect_url() {
        let base_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_addr = base_listener.local_addr().unwrap();
        let unused_redir = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let redir_addr = unused_redir.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (s1, _) = base_listener.accept().await.unwrap();
            let mut ws1 = tokio_tungstenite::accept_async(s1).await.unwrap();
            ws1.send(Message::text(r#"{"type":"hello"}"#))
                .await
                .unwrap();
            ws1.send(Message::text(format!(
                r#"{{"type":"reconnect_url","url":"ws://{redir_addr}/expired"}}"#
            )))
            .await
            .unwrap();
            ws1.send(Message::text(
                r#"{"type":"error","error":{"code":1,"msg":"Socket URL has expired"}}"#,
            ))
            .await
            .unwrap();
            ws1.send(Message::text(r#"{"type":"goodbye"}"#))
                .await
                .unwrap();
            let _ = ws1.close(None).await;

            // Because code 1 cleared reconnect_url, session 2 must hit base_listener directly.
            let (s2, _) = base_listener.accept().await.unwrap();
            let mut ws2 = tokio_tungstenite::accept_async(s2).await.unwrap();
            ws2.send(Message::text(r#"{"type":"hello"}"#))
                .await
                .unwrap();
            let _ = ws2.next().await;
        });

        let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-exp"));
        cfg.base_url = format!("ws://{base_addr}/link/");
        cfg.backoff = Backoff {
            initial: Duration::from_millis(10),
            max: Duration::from_millis(20),
        };
        let (client, mut rx, driver) = RtmClient::new(cfg).unwrap();
        let drv = tokio::spawn(driver);

        let mut connected = 0;
        for _ in 0..15 {
            if let RtmEvent::State(ConnectionState::Connected { .. }) = next_event(&mut rx).await {
                connected += 1;
                if connected == 2 {
                    break;
                }
            }
        }
        assert_eq!(connected, 2);

        client.close();
        let _ = tokio::time::timeout(Duration::from_secs(2), drv).await;
        server.await.unwrap();
    }

    #[tokio::test]
    async fn in_band_401_and_http_403_stop_while_502_and_503_retry() {
        // 1. In-band error code 401 -> terminal Disconnected
        {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (s, _) = listener.accept().await.unwrap();
                let mut ws = tokio_tungstenite::accept_async(s).await.unwrap();
                ws.send(Message::text(
                    r#"{"type":"error","error":{"code":401,"msg":"bad token"}}"#,
                ))
                .await
                .unwrap();
            });
            let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-bad"));
            cfg.base_url = format!("ws://{addr}/link/");
            let (_client, mut rx, driver) = RtmClient::new(cfg).unwrap();
            let drv = tokio::spawn(driver);

            let mut stopped = None;
            for _ in 0..5 {
                if let RtmEvent::State(ConnectionState::Disconnected { reason }) =
                    next_event(&mut rx).await
                {
                    stopped = Some(reason);
                    break;
                }
            }
            assert!(
                stopped
                    .as_deref()
                    .is_some_and(|r| r.contains("invalid auth")),
                "{stopped:?}"
            );
            tokio::time::timeout(Duration::from_secs(2), drv)
                .await
                .unwrap()
                .unwrap();
            server.await.unwrap();
        }

        // 2. HTTP 403 -> terminal Disconnected
        {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let _ = sock
                    .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                    .await;
                let _ = sock.shutdown().await;
            });
            let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-403"));
            cfg.base_url = format!("ws://{addr}/link/");
            let (_client, mut rx, driver) = RtmClient::new(cfg).unwrap();
            let drv = tokio::spawn(driver);

            assert_eq!(
                next_event(&mut rx).await,
                RtmEvent::State(ConnectionState::Connecting)
            );
            match next_event(&mut rx).await {
                RtmEvent::State(ConnectionState::Disconnected { reason }) => {
                    assert!(reason.contains("403"), "got: {reason}");
                }
                other => panic!("expected Disconnected on 403, got {other:?}"),
            }
            tokio::time::timeout(Duration::from_secs(2), drv)
                .await
                .unwrap()
                .unwrap();
            server.await.unwrap();
        }

        // 3. HTTP 502 then 503 then WebSocket success -> retryable reconnects
        {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                for status in ["502 Bad Gateway", "503 Service Unavailable"] {
                    let (mut sock, _) = listener.accept().await.unwrap();
                    let mut buf = [0u8; 1024];
                    let _ = sock.read(&mut buf).await;
                    let _ = sock
                        .write_all(
                            format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\n\r\n").as_bytes(),
                        )
                        .await;
                    let _ = sock.shutdown().await;
                }
                let (s, _) = listener.accept().await.unwrap();
                let mut ws = tokio_tungstenite::accept_async(s).await.unwrap();
                ws.send(Message::text(r#"{"type":"hello"}"#)).await.unwrap();
                let _ = ws.next().await;
            });

            let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-retry"));
            cfg.base_url = format!("ws://{addr}/link/");
            cfg.backoff = Backoff {
                initial: Duration::from_millis(10),
                max: Duration::from_millis(20),
            };
            let (client, mut rx, driver) = RtmClient::new(cfg).unwrap();
            let drv = tokio::spawn(driver);

            let mut attempts = Vec::new();
            let mut connected = false;
            for _ in 0..12 {
                match next_event(&mut rx).await {
                    RtmEvent::State(ConnectionState::Reconnecting { attempt, .. }) => {
                        attempts.push(attempt);
                    }
                    RtmEvent::State(ConnectionState::Connected { .. }) => {
                        connected = true;
                        break;
                    }
                    _ => {}
                }
            }
            assert_eq!(attempts, vec![1, 2]);
            assert!(connected);

            client.close();
            let _ = tokio::time::timeout(Duration::from_secs(2), drv).await;
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn handshake_sends_origin_cookie_and_user_agent_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (s, _) = listener.accept().await.unwrap();
            let mut seen_origin = None;
            let mut seen_cookie = None;
            let mut seen_ua = None;
            #[allow(clippy::result_large_err)]
            let mut ws = tokio_tungstenite::accept_hdr_async(
                s,
                |req: &tungstenite::handshake::server::Request,
                 resp: tungstenite::handshake::server::Response| {
                    seen_origin = req
                        .headers()
                        .get(ORIGIN)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned);
                    seen_cookie = req
                        .headers()
                        .get(COOKIE)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned);
                    seen_ua = req
                        .headers()
                        .get(USER_AGENT)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned);
                    Ok(resp)
                },
            )
            .await
            .unwrap();
            assert_eq!(seen_origin.as_deref(), Some(WEB_ORIGIN));
            assert_eq!(seen_cookie.as_deref(), Some("d=xoxd-cookie-val"));
            assert_eq!(seen_ua.as_deref(), Some("SlacklessTest/1.0"));
            ws.send(Message::text(r#"{"type":"hello"}"#)).await.unwrap();
            let _ = ws.next().await;
        });

        let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-hdr"));
        cfg.base_url = format!("ws://{addr}/link/");
        cfg.d_cookie = Some("xoxd-cookie-val".to_owned());
        cfg.user_agent = Some("SlacklessTest/1.0".to_owned());
        let (client, mut rx, driver) = RtmClient::new(cfg).unwrap();
        let drv = tokio::spawn(driver);

        for _ in 0..4 {
            if matches!(
                next_event(&mut rx).await,
                RtmEvent::State(ConnectionState::Connected { .. })
            ) {
                break;
            }
        }
        client.close();
        let _ = tokio::time::timeout(Duration::from_secs(2), drv).await;
        server.await.unwrap();
    }

    #[tokio::test]
    async fn close_aborts_backoff_wait_promptly() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf).await;
            let _ = sock
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n")
                .await;
            let _ = sock.shutdown().await;
        });

        let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-wait"));
        cfg.base_url = format!("ws://{addr}/link/");
        cfg.backoff = Backoff {
            initial: Duration::from_secs(30),
            max: Duration::from_secs(30),
        };
        let (client, mut rx, driver) = RtmClient::new(cfg).unwrap();
        let drv = tokio::spawn(driver);

        assert_eq!(
            next_event(&mut rx).await,
            RtmEvent::State(ConnectionState::Connecting)
        );
        assert!(matches!(
            next_event(&mut rx).await,
            RtmEvent::State(ConnectionState::Reconnecting { .. })
        ));

        let started = Instant::now();
        client.close();
        assert!(matches!(
            next_event(&mut rx).await,
            RtmEvent::State(ConnectionState::Disconnected { .. })
        ));
        tokio::time::timeout(Duration::from_secs(1), drv)
            .await
            .expect("close should abort long backoff immediately")
            .unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn presence_sub_during_backoff_replays_server_close_reconnects_healthy_resets_and_ids_monotonic()
     {
        use tokio_tungstenite::tungstenite::protocol::CloseFrame;
        use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            // Session 1: short-lived, receives typing (id=1) & tickle (id=2), closes with CloseFrame.
            let (s1, _) = listener.accept().await.unwrap();
            let mut ws1 = tokio_tungstenite::accept_async(s1).await.unwrap();
            ws1.send(Message::text(r#"{"type":"hello"}"#))
                .await
                .unwrap();
            let f1: serde_json::Value =
                serde_json::from_str(&ws1.next().await.unwrap().unwrap().into_text().unwrap())
                    .unwrap();
            let f2: serde_json::Value =
                serde_json::from_str(&ws1.next().await.unwrap().unwrap().into_text().unwrap())
                    .unwrap();
            assert_eq!(f1["id"], 1);
            assert_eq!(f2["id"], 2);
            ws1.send(Message::Close(Some(CloseFrame {
                code: CloseCode::Restart,
                reason: "restarting".into(),
            })))
            .await
            .unwrap();

            // Session 2: stays alive >= healthy_after (80ms), verifies presence_sub queued during
            // backoff replays with id=3, then closes with goodbye.
            let (s2, _) = listener.accept().await.unwrap();
            let mut ws2 = tokio_tungstenite::accept_async(s2).await.unwrap();
            ws2.send(Message::text(r#"{"type":"hello"}"#))
                .await
                .unwrap();
            let sub: serde_json::Value =
                serde_json::from_str(&ws2.next().await.unwrap().unwrap().into_text().unwrap())
                    .unwrap();
            assert_eq!(sub["type"], "presence_sub");
            assert_eq!(sub["ids"], serde_json::json!(["U999"]));
            assert_eq!(sub["id"], 3);

            tokio::time::sleep(Duration::from_millis(90)).await;
            ws2.send(Message::text(r#"{"type":"goodbye"}"#))
                .await
                .unwrap();
            let _ = ws2.close(None).await;

            // Session 3: receives replayed presence_sub with id=4 (monotonic across reconnects).
            let (s3, _) = listener.accept().await.unwrap();
            let mut ws3 = tokio_tungstenite::accept_async(s3).await.unwrap();
            ws3.send(Message::text(r#"{"type":"hello"}"#))
                .await
                .unwrap();
            let sub2: serde_json::Value =
                serde_json::from_str(&ws3.next().await.unwrap().unwrap().into_text().unwrap())
                    .unwrap();
            assert_eq!(sub2["type"], "presence_sub");
            assert_eq!(sub2["id"], 4);
            let _ = ws3.next().await;
        });

        let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-lifecycle"));
        cfg.base_url = format!("ws://{addr}/link/");
        cfg.healthy_after = Duration::from_millis(70);
        cfg.backoff = Backoff {
            initial: Duration::from_millis(40),
            max: Duration::from_millis(80),
        };
        let (client, mut rx, driver) = RtmClient::new(cfg).unwrap();
        let drv = tokio::spawn(driver);

        // Wait for Connected #1, send 2 frames (ids 1, 2).
        for _ in 0..4 {
            if matches!(
                next_event(&mut rx).await,
                RtmEvent::State(ConnectionState::Connected { .. })
            ) {
                break;
            }
        }
        client.user_typing(ChannelId::from("C1"), None).unwrap();
        client.tickle(TickleReason::Focus).unwrap();

        // Wait for Reconnecting #1 (from Message::Close), then queue presence_sub during backoff wait.
        let mut attempts = Vec::new();
        let mut saw_server_close = false;
        let mut connected = 1;
        for _ in 0..20 {
            match next_event(&mut rx).await {
                RtmEvent::State(ConnectionState::Reconnecting {
                    attempt, reason, ..
                }) => {
                    attempts.push(attempt);
                    if attempts.len() == 1 {
                        saw_server_close = reason.contains("closed by server");
                        client.presence_sub(vec![UserId::from("U999")]).unwrap();
                    }
                }
                RtmEvent::State(ConnectionState::Connected { .. }) => {
                    connected += 1;
                    if connected == 3 {
                        break;
                    }
                }
                _ => {}
            }
        }
        assert!(saw_server_close);
        // Attempt 1 after short session 1; after healthy session 2 (>= 70ms), attempt resets to 1!
        assert_eq!(attempts, vec![1, 1]);
        assert_eq!(connected, 3);

        client.close();
        let _ = tokio::time::timeout(Duration::from_secs(2), drv).await;
        server.await.unwrap();
    }
}
