//! Thin send-handle facade over the background websocket [`Driver`].

use std::future::Future;

use slackless_types::{ChannelId, Ts, UserId};
use tokio::sync::{mpsc, watch};

use crate::config::{RtmConfig, RtmError, RtmEvent};
use crate::driver::{Command, Driver};
use crate::frame::{Outgoing, TickleReason};
use crate::tls::tls_connector;

/// Cheap, cloneable handle for sending frames to the socket.
///
/// Frames sent while disconnected are dropped (typing and pings are stale by then); the last
/// `presence_sub` is re-sent automatically after every `hello`.
#[derive(Debug, Clone)]
pub struct RtmClient {
    commands: mpsc::Sender<Command>,
    close: watch::Sender<bool>,
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
        let (cmd_tx, cmd_rx) = mpsc::channel(config.command_buffer.max(1));
        let (close_tx, close_rx) = watch::channel(false);
        let (event_tx, event_rx) = mpsc::channel(config.event_buffer.max(1));
        let driver = Driver {
            tls: tls_connector()?,
            config,
            commands: cmd_rx,
            close: close_rx,
            events: event_tx,
            next_id: 1,
            reconnect_url: None,
            presence_sub: None,
        };
        Ok((
            Self {
                commands: cmd_tx,
                close: close_tx,
            },
            event_rx,
            driver.run(),
        ))
    }

    /// Queues a frame; it gets the next monotonic `id` when written.
    ///
    /// Returns [`RtmError::QueueFull`] if the bounded outgoing queue is full, or
    /// [`RtmError::Closed`] if the driver has shut down.
    pub fn send(&self, frame: Outgoing) -> Result<(), RtmError> {
        self.commands
            .try_send(Command::Send(frame))
            .map_err(|err| match err {
                mpsc::error::TrySendError::Full(_) => RtmError::QueueFull,
                mpsc::error::TrySendError::Closed(_) => RtmError::Closed,
            })
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
        let _ = self.close.send(true);
    }
}
