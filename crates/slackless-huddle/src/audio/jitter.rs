use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use bytes::Bytes;
use opus::Decoder;

use super::FRAME_20MS;

/// Target playback jitter-buffer pre-fill before releasing audio after startup or underrun
/// (60 ms = 3 × 20 ms Opus frames). Eliminates 512-sample CoreAudio vs 960-sample Opus beat
/// underruns and absorbs normal internet UDP jitter cleanly.
pub(super) const PLAYBACK_PRIME_SAMPLES: usize = FRAME_20MS * 3;
/// High-water mark above which excess queued playback audio is trimmed down cleanly (240 ms).
pub(super) const PLAYBACK_MAX_SAMPLES: usize = FRAME_20MS * 12;
/// Target sample depth after an overflow trim (80 ms).
pub(super) const PLAYBACK_TRIM_TARGET: usize = FRAME_20MS * 4;

/// Smooth, glitch-free playback ring buffer shared between the dedicated inbound Opus decoder
/// thread (`push`) and the real-time CoreAudio / `cpal` render callback (`pull_into`).
pub(crate) struct PlaybackBuffer {
    pub(super) samples: VecDeque<f32>,
    pub(super) primed: bool,
    last_sample: f32,
}

impl PlaybackBuffer {
    pub(crate) fn new() -> Self {
        Self {
            samples: VecDeque::with_capacity(PLAYBACK_MAX_SAMPLES),
            primed: false,
            last_sample: 0.0,
        }
    }

    pub(crate) fn push(&mut self, pcm: &[f32]) {
        if self.samples.len() + pcm.len() > PLAYBACK_MAX_SAMPLES {
            let keep = PLAYBACK_TRIM_TARGET.min(self.samples.len());
            let drop_n = self.samples.len().saturating_sub(keep);
            if drop_n > 0 {
                let prev_end = self.samples.front().copied().unwrap_or(self.last_sample);
                self.samples.drain(..drop_n);
                // Short 32-sample crossfade at the splice boundary avoids an audible pop.
                let fade = 32.min(self.samples.len());
                for i in 0..fade {
                    let t = (i + 1) as f32 / (fade + 1) as f32;
                    if let Some(s) = self.samples.get_mut(i) {
                        *s = prev_end * (1.0 - t) + *s * t;
                    }
                }
            }
        }
        self.samples.extend(pcm.iter().copied());
        if !self.primed && self.samples.len() >= PLAYBACK_PRIME_SAMPLES {
            self.primed = true;
        }
    }

    pub(crate) fn pull_into(&mut self, out: &mut [f32]) {
        if !self.primed {
            self.fade_to_silence(out);
            return;
        }
        let avail = self.samples.len().min(out.len());
        for dst in out.iter_mut().take(avail) {
            let s = self.samples.pop_front().unwrap_or(0.0).clamp(-1.0, 1.0);
            *dst = s;
            self.last_sample = s;
        }
        if avail < out.len() {
            // Genuine network underrun: ramp smoothly to zero and re-prime before resuming so
            // playback never sputters 1 frame at a time.
            self.primed = false;
            self.fade_to_silence(&mut out[avail..]);
        }
    }

    fn fade_to_silence(&mut self, out: &mut [f32]) {
        let mut s = self.last_sample;
        for dst in out.iter_mut() {
            s *= 0.85;
            if s.abs() < 1e-5 {
                s = 0.0;
            }
            *dst = s;
        }
        self.last_sample = s;
    }
}

/// Signed modular difference `a - b` in 16-bit RTP sequence space (positive means `a` is newer).
#[inline]
fn seq_diff(a: u16, b: u16) -> i32 {
    a.wrapping_sub(b) as i16 as i32
}

/// Sequence-ordered Opus RTP jitter buffer supporting out-of-order UDP delivery, Opus in-band FEC
/// recovery for single lost packets, and Opus PLC fallback for multi-packet loss.
pub(crate) struct AudioJitterBuffer {
    pub(super) queue: BTreeMap<i64, Bytes>,
    /// Extended 64-bit sequence number of the next expected packet.
    pub(super) next_seq: Option<i64>,
    /// Highest extended 64-bit sequence number observed so far (for wraparound unwrapping).
    base_seq: Option<i64>,
    missing_since: Option<Instant>,
    consecutive_plc: u8,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum JitterStep {
    /// Decode this normal Opus packet.
    Packet(Bytes),
    /// Missing packet whose immediate successor is already buffered: decode successor with
    /// Opus in-band FEC (`decode_fec = true`) to recover the lost frame cleanly.
    FecFromNext(Bytes),
    /// Missing packet with no immediate successor available: invoke Opus PLC.
    Plc,
}

impl AudioJitterBuffer {
    pub(crate) fn new() -> Self {
        Self {
            queue: BTreeMap::new(),
            next_seq: None,
            base_seq: None,
            missing_since: None,
            consecutive_plc: 0,
        }
    }

    fn unwrap_seq(&mut self, seq16: u16) -> i64 {
        match self.base_seq {
            None => {
                let s = seq16 as i64;
                self.base_seq = Some(s);
                s
            }
            Some(base) => {
                let diff = seq_diff(seq16, (base & 0xFFFF) as u16) as i64;
                let ext = base + diff;
                if ext > base {
                    self.base_seq = Some(ext);
                }
                ext
            }
        }
    }

    pub(crate) fn push(&mut self, seq16: u16, payload: Bytes, now: Instant) {
        let ext = self.unwrap_seq(seq16);
        let next = match self.next_seq {
            Some(n) => n,
            None => {
                self.next_seq = Some(ext);
                ext
            }
        };
        // Ignore stale packets already played out.
        if ext < next {
            return;
        }
        // Large jump (e.g. speaker resumed after DTX silence or SSRC switch): resync cleanly.
        if ext - next > 20 {
            self.queue.clear();
            self.next_seq = Some(ext);
            self.missing_since = None;
            self.consecutive_plc = 0;
        }
        if ext > next && self.missing_since.is_none() {
            self.missing_since = Some(now);
        }
        self.queue.insert(ext, payload);
        // Cap queue depth to prevent unbounded growth during stalls.
        while self.queue.len() > 24 {
            if let Some((&oldest, _)) = self.queue.iter().next() {
                self.queue.remove(&oldest);
                if self.next_seq.is_some_and(|n| oldest >= n) {
                    self.next_seq = Some(oldest + 1);
                }
            }
        }
    }

    /// Pop the next ready packet/FEC/PLC step if available at `now`.
    pub(crate) fn pop_ready(&mut self, now: Instant) -> Option<JitterStep> {
        let next = self.next_seq?;
        if let Some(pkt) = self.queue.remove(&next) {
            self.next_seq = Some(next + 1);
            self.missing_since = if self.queue.is_empty() || self.queue.contains_key(&(next + 1)) {
                None
            } else {
                Some(now)
            };
            self.consecutive_plc = 0;
            return Some(JitterStep::Packet(pkt));
        }

        // Next expected sequence number is missing — wait up to 35ms (or 2 newer packets) so
        // slightly reordered UDP packets can arrive before triggering FEC/PLC.
        let (&first_avail, _) = self.queue.iter().next()?;
        let wait_elapsed = self
            .missing_since
            .is_some_and(|t| now.saturating_duration_since(t) >= Duration::from_millis(35));
        let enough_newer = self.queue.len() >= 2 && first_avail - next <= 4;
        if !wait_elapsed && !enough_newer {
            return None;
        }

        // If gap is large (> 4 frames), jump directly to the next available packet (likely DTX).
        if first_avail - next > 4 || self.consecutive_plc >= 4 {
            self.next_seq = Some(first_avail);
            self.missing_since = None;
            self.consecutive_plc = 0;
            return self.pop_ready(now);
        }

        self.next_seq = Some(next + 1);
        self.consecutive_plc = self.consecutive_plc.saturating_add(1);
        if let Some(next_pkt) = self.queue.get(&(next + 1)).cloned() {
            Some(JitterStep::FecFromNext(next_pkt))
        } else {
            Some(JitterStep::Plc)
        }
    }
}

/// Decode a received Opus packet, or invoke Opus Packet Loss Concealment (PLC) when `pkt` is
/// empty (a sentinel emitted by the RTP reader when 1..=4 consecutive frames were dropped).
pub(crate) fn decode_opus_or_plc(
    decoder: &mut Decoder,
    pkt: &[u8],
    out: &mut [f32],
) -> std::result::Result<usize, opus::Error> {
    if pkt.is_empty() {
        let plc_len = FRAME_20MS.min(out.len());
        decoder.decode_float(&[], &mut out[..plc_len], false)
    } else {
        decoder.decode_float(pkt, out, false)
    }
}
