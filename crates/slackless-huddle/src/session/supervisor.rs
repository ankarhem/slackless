use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc;

use super::connected::run_connected_loop;
use crate::{
    HuddleAudioConfig, HuddleControl, HuddleCredentials, HuddleError, HuddleHandle, HuddlePhase,
    HuddleSessionSnapshot, HuddleUpdate,
};

pub(super) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub(super) struct SessionDoneGuard {
    pub(super) finished: Arc<AtomicBool>,
    pub(super) done: Arc<tokio::sync::Notify>,
}

impl Drop for SessionDoneGuard {
    fn drop(&mut self) {
        self.finished.store(true, Ordering::Release);
        self.done.notify_waiters();
    }
}

/// Start a native audio/video/screen-share huddle session on the current tokio runtime.
pub fn spawn_huddle_session(
    creds: HuddleCredentials,
    config: HuddleAudioConfig,
    updates: mpsc::UnboundedSender<HuddleUpdate>,
) -> HuddleHandle {
    let (ctrl_tx, ctrl_rx) = mpsc::unbounded_channel();
    let finished = Arc::new(AtomicBool::new(false));
    let done = Arc::new(tokio::sync::Notify::new());
    let guard = SessionDoneGuard {
        finished: Arc::clone(&finished),
        done: Arc::clone(&done),
    };
    tokio::spawn(async move {
        let _done_guard = guard;
        run_session(creds, config, ctrl_rx, updates).await;
    });
    HuddleHandle::new(ctrl_tx, finished, done)
}

#[derive(Debug)]
pub(super) enum LoopExit {
    LeftCleanly,
    Reconnect {
        err: HuddleError,
        /// `true` when this attempt reached `Connected` before dropping, resetting retry budget.
        was_connected: bool,
    },
    Fatal(HuddleError),
}

const MAX_RECONNECTS: u32 = 3;

/// Classify a signaling/handshake failure: on reconnect attempts (`attempt > 0`), treat transient
/// network/signaling/WebRTC errors as retryable `Reconnect` so retries #2 and #3 actually run.
pub(super) fn classify_handshake_err(err: HuddleError, is_reconnect: bool) -> LoopExit {
    if is_reconnect
        && !matches!(
            err,
            HuddleError::Protocol { status, .. } if (400..500).contains(&status)
        )
    {
        LoopExit::Reconnect {
            err,
            was_connected: false,
        }
    } else {
        LoopExit::Fatal(err)
    }
}

/// Classify an unexpected clean close (`Ok(None)`) from `signal.next_frame()`.
pub(super) fn classify_signal_close(ever_connected: bool, is_reconnect: bool) -> LoopExit {
    let err = HuddleError::Signaling("signaling socket closed unexpectedly".to_owned());
    if ever_connected || is_reconnect {
        LoopExit::Reconnect {
            err,
            was_connected: ever_connected,
        }
    } else {
        LoopExit::Fatal(err)
    }
}

pub(super) trait ReconnectRunner {
    fn run_attempt<'a>(
        &'a mut self,
        attempt: u32,
        snap: &'a mut HuddleSessionSnapshot,
        ctrl_rx: &'a mut mpsc::UnboundedReceiver<HuddleControl>,
        pending_controls: &'a mut VecDeque<HuddleControl>,
    ) -> Pin<Box<dyn Future<Output = LoopExit> + Send + 'a>>;
}

struct LiveSessionRunner<'a> {
    creds: &'a HuddleCredentials,
    config: &'a mut HuddleAudioConfig,
    updates: &'a mpsc::UnboundedSender<HuddleUpdate>,
}

impl ReconnectRunner for LiveSessionRunner<'_> {
    fn run_attempt<'a>(
        &'a mut self,
        attempt: u32,
        snap: &'a mut HuddleSessionSnapshot,
        ctrl_rx: &'a mut mpsc::UnboundedReceiver<HuddleControl>,
        pending_controls: &'a mut VecDeque<HuddleControl>,
    ) -> Pin<Box<dyn Future<Output = LoopExit> + Send + 'a>> {
        Box::pin(run_connected_loop(
            self.creds,
            self.config,
            snap,
            ctrl_rx,
            pending_controls,
            self.updates,
            attempt > 0,
        ))
    }
}

pub(super) async fn run_reconnect_supervisor(
    runner: &mut impl ReconnectRunner,
    snap: &mut HuddleSessionSnapshot,
    ctrl_rx: &mut mpsc::UnboundedReceiver<HuddleControl>,
    updates: &mpsc::UnboundedSender<HuddleUpdate>,
) {
    let mut attempt = 0u32;
    let mut pending_controls: VecDeque<HuddleControl> = VecDeque::new();

    let final_err = loop {
        match runner
            .run_attempt(attempt, snap, ctrl_rx, &mut pending_controls)
            .await
        {
            LoopExit::LeftCleanly => break None,
            LoopExit::Fatal(err) => break Some(err),
            LoopExit::Reconnect { err, was_connected } => {
                if was_connected {
                    attempt = 0;
                }
                attempt += 1;
                if attempt > MAX_RECONNECTS {
                    break Some(err);
                }
                snap.phase = HuddlePhase::Reconnecting;
                snap.error = None;
                let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
                let backoff = Duration::from_secs(1u64 << (attempt - 1));
                let sleep = tokio::time::sleep(backoff);
                tokio::pin!(sleep);
                let mut cancelled = false;
                loop {
                    tokio::select! {
                        () = &mut sleep => break,
                        ctrl = ctrl_rx.recv() => {
                            match ctrl {
                                Some(HuddleControl::Leave) | None => {
                                    cancelled = true;
                                    break;
                                }
                                Some(other) => {
                                    pending_controls.push_back(other);
                                }
                            }
                        }
                    }
                }
                if cancelled {
                    break None;
                }
            }
        }
    };

    match final_err {
        None => {
            snap.phase = HuddlePhase::Idle;
            snap.video = false;
            snap.sharing = false;
            snap.jukebox_song = None;
            snap.error = None;
            let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
        }
        Some(err) => {
            snap.phase = HuddlePhase::Failed;
            snap.video = false;
            snap.sharing = false;
            snap.jukebox_song = None;
            snap.udp_blocked = matches!(err, HuddleError::UdpBlocked);
            snap.error = Some(err.to_string());
            let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));
        }
    }
}

async fn run_session(
    creds: HuddleCredentials,
    mut config: HuddleAudioConfig,
    mut ctrl_rx: mpsc::UnboundedReceiver<HuddleControl>,
    updates: mpsc::UnboundedSender<HuddleUpdate>,
) {
    let mut snap = HuddleSessionSnapshot {
        phase: HuddlePhase::Connecting,
        channel: creds.channel.clone(),
        call_id: creds.call_id.clone(),
        attendee_id: creds.attendee_id.clone(),
        muted: config.muted,
        blur_background: config.blur_background,
        started_at_secs: now_secs(),
        thread_root_ts: creds.thread_root_ts.clone(),
        canvas_file_id: creds.canvas_file_id.clone(),
        huddle_link: creds.huddle_link.clone(),
        input_device: config.input_device.clone(),
        output_device: config.output_device.clone(),
        aec: config.aec,
        ns: config.ns,
        agc: config.agc,
        ..Default::default()
    };
    let _ = updates.send(HuddleUpdate::State(Box::new(snap.clone())));

    let mut runner = LiveSessionRunner {
        creds: &creds,
        config: &mut config,
        updates: &updates,
    };
    run_reconnect_supervisor(&mut runner, &mut snap, &mut ctrl_rx, &updates).await;
}
