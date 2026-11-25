use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use opus::Decoder;

use super::FRAME_20MS;
use super::ring::SpscRing;

/// Target playback jitter-buffer pre-fill before releasing audio after startup or underrun
/// (60 ms = 3 × 20 ms Opus frames). Eliminates 512-sample CoreAudio vs 960-sample Opus beat
/// underruns and absorbs normal internet UDP jitter cleanly.
pub(super) const PLAYBACK_PRIME_SAMPLES: usize = FRAME_20MS * 3;
/// High-water mark above which excess queued playback audio is trimmed down cleanly (240 ms).
pub(super) const PLAYBACK_MAX_SAMPLES: usize = FRAME_20MS * 12;
/// Target sample depth after an overflow trim (80 ms).
pub(super) const PLAYBACK_TRIM_TARGET: usize = FRAME_20MS * 4;
/// Underlying SPSC ring capacity (power of two > `PLAYBACK_MAX_SAMPLES` to absorb bursty pushes
/// before consumer-side trimming runs).
const PLAYBACK_RING_CAPACITY: usize = 32_768;
/// Crossfade length at overflow trim splice boundaries to eliminate audible clicks.
const SPLICE_FADE_SAMPLES: usize = 32;

/// Lock-free, glitch-free playback ring buffer shared between the dedicated inbound Opus decoder
/// thread (`push`) and the real-time CoreAudio / `cpal` render callback (`pull_into`).
///
/// Neither side ever locks a mutex or allocates on the heap.
pub(crate) struct PlaybackBuffer {
    ring: SpscRing<f32>,
    primed: AtomicBool,
    last_sample_bits: AtomicU32,
    splice_from_bits: AtomicU32,
    splice_pos: AtomicUsize,
}

impl PlaybackBuffer {
    pub(crate) fn new() -> Self {
        Self {
            ring: SpscRing::new(PLAYBACK_RING_CAPACITY),
            primed: AtomicBool::new(false),
            last_sample_bits: AtomicU32::new(0.0f32.to_bits()),
            splice_from_bits: AtomicU32::new(0.0f32.to_bits()),
            splice_pos: AtomicUsize::new(SPLICE_FADE_SAMPLES),
        }
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.ring.len()
    }

    #[cfg(test)]
    pub(super) fn is_primed(&self) -> bool {
        self.primed.load(Ordering::Acquire)
    }

    #[inline]
    fn last_sample(&self) -> f32 {
        f32::from_bits(self.last_sample_bits.load(Ordering::Relaxed))
    }

    /// Producer (inbound Opus decoder thread): append decoded PCM samples without blocking.
    pub(crate) fn push(&self, pcm: &[f32]) {
        self.ring.push_slice(pcm);
        if !self.primed.load(Ordering::Acquire) && self.ring.len() >= PLAYBACK_PRIME_SAMPLES {
            self.primed.store(true, Ordering::Release);
        }
    }

    /// Consumer (real-time audio render callback): pull PCM samples into `out` without locks or
    /// allocations, performing consumer-side overflow trimming with a 32-sample crossfade splice
    /// and smooth fade-to-silence on underrun.
    pub(crate) fn pull_into(&self, out: &mut [f32]) {
        if !self.primed.load(Ordering::Acquire) {
            self.fade_to_silence(out);
            return;
        }
        let cur_len = self.ring.len();
        if cur_len > PLAYBACK_MAX_SAMPLES {
            let keep = PLAYBACK_TRIM_TARGET.min(cur_len);
            let drop_n = cur_len.saturating_sub(keep);
            if drop_n > 0 {
                let prev_end = self.ring.peek_front().unwrap_or_else(|| self.last_sample());
                self.ring.skip(drop_n);
                self.splice_from_bits
                    .store(prev_end.to_bits(), Ordering::Relaxed);
                self.splice_pos.store(0, Ordering::Relaxed);
            }
        }

        let avail = self.ring.pop_slice(out);
        let mut last = self.last_sample();
        let mut splice_pos = self.splice_pos.load(Ordering::Relaxed);
        let splice_from = f32::from_bits(self.splice_from_bits.load(Ordering::Relaxed));
        for dst in out.iter_mut().take(avail) {
            let mut s = *dst;
            if splice_pos < SPLICE_FADE_SAMPLES {
                let t = (splice_pos + 1) as f32 / (SPLICE_FADE_SAMPLES + 1) as f32;
                s = splice_from * (1.0 - t) + s * t;
                splice_pos += 1;
            }
            s = s.clamp(-1.0, 1.0);
            *dst = s;
            last = s;
        }
        self.splice_pos.store(splice_pos, Ordering::Relaxed);
        self.last_sample_bits
            .store(last.to_bits(), Ordering::Relaxed);

        if avail < out.len() {
            // Genuine network underrun: ramp smoothly to zero and re-prime before resuming so
            // playback never sputters 1 frame at a time.
            self.primed.store(false, Ordering::Release);
            self.fade_to_silence(&mut out[avail..]);
        }
    }

    fn fade_to_silence(&self, out: &mut [f32]) {
        let mut s = self.last_sample();
        for dst in out.iter_mut() {
            s *= 0.85;
            if s.abs() < 1e-5 {
                s = 0.0;
            }
            *dst = s;
        }
        self.last_sample_bits.store(s.to_bits(), Ordering::Relaxed);
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
