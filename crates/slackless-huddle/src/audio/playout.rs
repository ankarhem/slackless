//! Remote audio receive and playout.
//!
//! Every remote SSRC gets its own WebRTC NetEq (the `neteq` crate: jitter buffer, loss
//! concealment, time stretching). Packets are inserted as they arrive; audio leaves in 10 ms
//! frames pulled at the output device's own rate, so NetEq's accelerate / preemptive expand absorb
//! both network jitter and the drift between the sender's clock and the device clock.
//!
//! ```text
//! RTP reader --mpsc--> playout thread: NetEq per SSRC -> mix --PlayoutRing--> render callback
//! ```
//!
//! NetEq allocates, so it lives on the playout thread. The render callback only pops from a
//! lock-free ring and wakes that thread to top the ring back up.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use neteq::api::{
    AudioDecoder, AudioDecoderFactory, Clock, DecodeResult, EncodedAudioFrame, ParseResult,
    SdpAudioFormat, SpeechType,
};
use neteq::{AudioFrame, Config, NetEq, RtpHeader, RtpPacketInfo};
use tokio::sync::mpsc;

use super::dsp::LoudnessNormalizer;
use super::ring::{SpscRing, WorkerWake};
use super::{FRAME_10MS, SAMPLE_RATE};
use crate::rtc::InboundAudioPacket;

/// Upper bound for NetEq's adaptive target delay. A conversation is unusable beyond this, so
/// concealing a late packet beats buffering even more.
pub(crate) const MAX_JITTER_DELAY_MS: i32 = 500;
/// Payload type Opus is registered under inside NetEq. The wire value does not matter: the audio
/// track only negotiates Opus.
const NETEQ_OPUS_PT: u8 = 111;
/// Longest Opus packet (120 ms).
const MAX_OPUS_SAMPLES: usize = FRAME_10MS * 12;
/// Streams mixed at once; the stalest one makes room for a new SSRC.
const MAX_STREAMS: usize = 8;
/// A stream without packets for this many 10 ms ticks (10 s) is dropped when a new SSRC shows up.
const STREAM_IDLE_TICKS: u64 = 1_000;
/// Playout ring capacity in samples (170 ms): room for large device callbacks.
const RING_CAPACITY: usize = 8_192;

// ---------------------------------------------------------------------------
// libopus behind NetEq's decoder interface
// ---------------------------------------------------------------------------

/// Samples of in-band FEC (LBRR) this packet carries for the frame before it, 0 if none.
///
/// RFC 6716 section 4.2.3/4.2.4: a SILK or hybrid frame starts with one VAD bit per SILK frame and
/// then the LBRR flag, per channel, as the first bits of the first frame (same test as WebRTC's
/// `WebRtcOpus_PacketHasFec`).
fn fec_samples(payload: &[u8]) -> usize {
    let Some(&toc) = payload.first() else {
        return 0;
    };
    if toc & 0x80 != 0 {
        return 0; // CELT-only frames carry no LBRR.
    }
    let Ok(samples) = opus::packet::get_samples_per_frame(payload, SAMPLE_RATE) else {
        return 0;
    };
    let silk_frames = match samples / FRAME_10MS {
        1 | 2 => 1,
        4 => 2,
        6 => 3,
        _ => return 0,
    };
    // A code-0 packet is the TOC followed by its single frame.
    let first_byte = if toc & 0x03 == 0 {
        payload.get(1).copied()
    } else {
        opus::packet::parse(payload)
            .ok()
            .and_then(|p| p.frames.first().and_then(|f| f.first().copied()))
    };
    let channels = if toc & 0x04 != 0 { 2 } else { 1 };
    let has_lbrr = first_byte.is_some_and(|byte| {
        (1..=channels).any(|ch| byte & (0x80 >> (ch * (silk_frames + 1) - 1)) != 0)
    });
    if has_lbrr { samples } else { 0 }
}

/// Samples in `payload`, 0 if it is not a valid Opus packet.
fn packet_samples(payload: &[u8]) -> usize {
    opus::packet::get_nb_samples(payload, SAMPLE_RATE)
        .ok()
        .filter(|&n| n <= MAX_OPUS_SAMPLES)
        .unwrap_or(0)
}

/// One Opus packet in NetEq's packet buffer: either the packet itself or the FEC copy of the
/// frame before it (which NetEq only decodes when that frame never arrived).
struct OpusFrame {
    payload: Bytes,
    samples: usize,
    fec: bool,
}

impl EncodedAudioFrame for OpusFrame {
    fn duration(&self) -> usize {
        self.samples
    }

    fn is_dtx_packet(&self) -> bool {
        self.payload.len() <= 2
    }

    fn decode(&self, decoder: &mut dyn AudioDecoder, decoded: &mut [i16]) -> Option<DecodeResult> {
        let mut speech_type = SpeechType::Speech;
        let rate = decoder.sample_rate_hz();
        let n = if self.fec {
            decoder.decode_redundant(&self.payload, rate, decoded, &mut speech_type)
        } else {
            decoder.decode(&self.payload, rate, decoded, &mut speech_type)
        };
        Some(DecodeResult {
            num_decoded_samples: usize::try_from(n).ok()?,
            speech_type,
        })
    }
}

/// NetEq decoder on the libopus the encoder already uses, so the binary carries one Opus
/// implementation. Mirrors WebRTC's `AudioDecoderOpus`: mono 48 kHz, FEC exposed as a
/// lower-priority frame, payloads of at most 2 bytes are DTX, and an empty payload asks for
/// 10 ms of codec concealment or comfort noise.
struct LibopusDecoder {
    decoder: opus::Decoder,
    in_dtx: bool,
}

impl LibopusDecoder {
    fn finish(
        &mut self,
        decoded: opus::Result<usize>,
        encoded_len: usize,
        speech_type: &mut SpeechType,
    ) -> i32 {
        let Ok(samples @ 1..) = decoded else {
            return -1;
        };
        // A DTX packet starts comfort noise; concealment calls (empty payload) continue it.
        self.in_dtx = match encoded_len {
            0 => self.in_dtx,
            1 | 2 => true,
            _ => false,
        };
        *speech_type = if self.in_dtx {
            SpeechType::ComfortNoise
        } else {
            SpeechType::Speech
        };
        i32::try_from(samples).unwrap_or(-1)
    }
}

impl AudioDecoder for LibopusDecoder {
    fn parse_payload(&mut self, payload: Vec<u8>, timestamp: u32) -> Vec<ParseResult> {
        let payload = Bytes::from(payload);
        let mut frames = Vec::with_capacity(2);
        let fec = fec_samples(&payload);
        if fec > 0 {
            let frame = OpusFrame {
                payload: payload.clone(),
                samples: fec,
                fec: true,
            };
            frames.push(ParseResult::new(
                timestamp.wrapping_sub(fec as u32),
                1,
                Box::new(frame),
            ));
        }
        let frame = OpusFrame {
            samples: packet_samples(&payload),
            payload,
            fec: false,
        };
        frames.push(ParseResult::new(timestamp, 0, Box::new(frame)));
        frames
    }

    fn decode_internal(
        &mut self,
        encoded: &[u8],
        _sample_rate_hz: i32,
        decoded: &mut [i16],
        speech_type: &mut SpeechType,
    ) -> i32 {
        let room = if encoded.is_empty() {
            FRAME_10MS
        } else {
            MAX_OPUS_SAMPLES
        };
        let room = room.min(decoded.len());
        let res = self.decoder.decode(encoded, &mut decoded[..room], false);
        self.finish(res, encoded.len(), speech_type)
    }

    fn decode_redundant_internal(
        &mut self,
        encoded: &[u8],
        _sample_rate_hz: i32,
        decoded: &mut [i16],
        speech_type: &mut SpeechType,
    ) -> i32 {
        let fec = fec_samples(encoded).min(decoded.len());
        let res = self.decoder.decode(encoded, &mut decoded[..fec], true);
        self.finish(res, encoded.len(), speech_type)
    }

    fn reset(&mut self) {
        let _ = self.decoder.reset_state();
        self.in_dtx = false;
    }

    fn sample_rate_hz(&self) -> i32 {
        SAMPLE_RATE as i32
    }

    fn channels(&self) -> usize {
        1
    }
}

struct LibopusFactory;

impl AudioDecoderFactory for LibopusFactory {
    fn make_decoder(&self, format: &SdpAudioFormat) -> Option<Box<dyn AudioDecoder>> {
        if !format.name.eq_ignore_ascii_case("opus") {
            return None;
        }
        let decoder = opus::Decoder::new(SAMPLE_RATE, opus::Channels::Mono).ok()?;
        Some(Box::new(LibopusDecoder {
            decoder,
            in_dtx: false,
        }))
    }
}

/// Monotonic NetEq clock counting from the mixer's epoch (packet arrival stamps use the same).
pub(crate) struct EpochClock(pub(crate) Instant);

impl Clock for EpochClock {
    fn current_time_us(&self) -> i64 {
        i64::try_from(self.0.elapsed().as_micros()).unwrap_or(i64::MAX)
    }
}

// ---------------------------------------------------------------------------
// Telemetry
// ---------------------------------------------------------------------------

/// Cumulative remote-audio receive/playout counters since the audio engine started, plus the
/// current delay gauges. The session logs the change between two snapshots in `huddle perf`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AudioRxSnapshot {
    /// Opus packets handed to NetEq.
    pub packets: u64,
    /// Padding-only RTP packets skipped (never decoded, never counted as loss).
    pub padding: u64,
    /// Packets NetEq rejected plus failed 10 ms pulls.
    pub errors: u64,
    /// Samples played out at 48 kHz, concealment included.
    pub played_samples: u64,
    /// Synthesized samples covering audio that was not there in time (expand / codec PLC).
    pub concealed_samples: u64,
    /// The part of `concealed_samples` that is plain silence: nothing arrived for a while.
    pub silent_concealed_samples: u64,
    /// Samples removed by accelerate (buffer above target, or the sender's clock runs fast).
    pub accelerated_samples: u64,
    /// Samples inserted by preemptive expand (buffer below target, or the sender runs slow).
    pub decelerated_samples: u64,
    /// Render callbacks that found the playout ring short.
    pub underruns: u64,
    /// NetEq's target jitter-buffer delay (largest across streams).
    pub target_delay_ms: u32,
    /// NetEq's filtered current jitter-buffer delay (largest across streams).
    pub current_delay_ms: u32,
    /// Remote streams currently mixed.
    pub streams: u32,
}

impl AudioRxSnapshot {
    /// Counter deltas since `earlier`; the gauges keep their current value. Saturating, because
    /// the counters restart with the audio engine (device switch).
    pub fn since(&self, earlier: &Self) -> Self {
        let delta = |now: u64, then: u64| now.saturating_sub(then);
        Self {
            packets: delta(self.packets, earlier.packets),
            padding: delta(self.padding, earlier.padding),
            errors: delta(self.errors, earlier.errors),
            played_samples: delta(self.played_samples, earlier.played_samples),
            concealed_samples: delta(self.concealed_samples, earlier.concealed_samples),
            silent_concealed_samples: delta(
                self.silent_concealed_samples,
                earlier.silent_concealed_samples,
            ),
            accelerated_samples: delta(self.accelerated_samples, earlier.accelerated_samples),
            decelerated_samples: delta(self.decelerated_samples, earlier.decelerated_samples),
            underruns: delta(self.underruns, earlier.underruns),
            ..*self
        }
    }

    /// Concealment the listener hears as stretched or synthetic speech, as opposed to silence.
    pub fn audible_concealed_samples(&self) -> u64 {
        self.concealed_samples
            .saturating_sub(self.silent_concealed_samples)
    }

    /// `samples` as a percentage of the samples played out.
    pub fn pct_of_played(&self, samples: u64) -> f64 {
        samples as f64 * 100.0 / self.played_samples.max(1) as f64
    }
}

// ---------------------------------------------------------------------------
// NetEq per SSRC + mix
// ---------------------------------------------------------------------------

struct RemoteStream {
    ssrc: u32,
    neteq: NetEq,
    normalizer: LoudnessNormalizer,
    last_packet_tick: u64,
}

impl RemoteStream {
    fn add_stats(&self, acc: &mut AudioRxSnapshot) {
        let life = self.neteq.lifetime_statistics();
        acc.played_samples += life.total_samples_received;
        acc.concealed_samples += life.concealed_samples;
        acc.silent_concealed_samples += life.silent_concealed_samples;
        acc.accelerated_samples += life.removed_samples_for_acceleration;
        acc.decelerated_samples += life.inserted_samples_for_deceleration;
    }
}

/// Jitter-buffers, decodes and mixes all remote audio streams of one connection.
pub(crate) struct RemoteAudioMixer {
    streams: Vec<RemoteStream>,
    frame: AudioFrame,
    scratch: [f32; FRAME_10MS],
    epoch: Instant,
    clock: Arc<dyn Clock>,
    tick: u64,
    /// Counters so far, including those of streams that were dropped.
    totals: AudioRxSnapshot,
}

impl RemoteAudioMixer {
    /// `epoch` is the zero point for packet arrival stamps and must match `clock`.
    pub(crate) fn new(epoch: Instant, clock: Arc<dyn Clock>) -> Self {
        Self {
            streams: Vec::new(),
            frame: AudioFrame::new(),
            scratch: [0.0; FRAME_10MS],
            epoch,
            clock,
            tick: 0,
            totals: AudioRxSnapshot::default(),
        }
    }

    /// Hand one received RTP packet to its stream's jitter buffer.
    pub(crate) fn insert(&mut self, pkt: &InboundAudioPacket) {
        if pkt.payload.is_empty() {
            // Padding-only packet (transport-cc bandwidth probing): no media, the timestamp of
            // the frame it follows, its own sequence number. NetEq orders and detects loss by
            // timestamp, so skipping it is all there is to do; it must never look like loss.
            self.totals.padding += 1;
            return;
        }
        let idx = self.stream_index(pkt.ssrc);
        let stream = &mut self.streams[idx];
        stream.last_packet_tick = self.tick;
        let header = RtpHeader {
            marker_bit: pkt.marker,
            payload_type: NETEQ_OPUS_PT,
            sequence_number: pkt.seq,
            timestamp: pkt.timestamp,
            ssrc: pkt.ssrc,
        };
        let arrival = pkt.arrival.saturating_duration_since(self.epoch);
        let info = RtpPacketInfo::new(&header, i64::try_from(arrival.as_micros()).ok());
        match stream.neteq.insert_packet(&header, &pkt.payload, info) {
            Ok(()) => self.totals.packets += 1,
            Err(_) => self.totals.errors += 1,
        }
    }

    fn stream_index(&mut self, ssrc: u32) -> usize {
        if let Some(idx) = self.streams.iter().position(|s| s.ssrc == ssrc) {
            return idx;
        }
        // A new SSRC (renegotiation, another sender): drop streams that went quiet so they stop
        // costing a pull per tick, and never mix more than `MAX_STREAMS`.
        let (tick, totals) = (self.tick, &mut self.totals);
        self.streams.retain(|s| {
            let live = tick - s.last_packet_tick <= STREAM_IDLE_TICKS;
            if !live {
                s.add_stats(totals);
            }
            live
        });
        if self.streams.len() >= MAX_STREAMS
            && let Some(stalest) =
                (0..self.streams.len()).min_by_key(|&i| self.streams[i].last_packet_tick)
        {
            self.streams
                .swap_remove(stalest)
                .add_stats(&mut self.totals);
        }

        let config = Config {
            sample_rate_hz: SAMPLE_RATE as i32,
            max_delay_ms: MAX_JITTER_DELAY_MS,
            // Once concealment has faded to silence NetEq returns muted frames without doing work.
            enable_muted_state: true,
            ..Config::default()
        };
        let mut neteq = NetEq::builder(config)
            .decoder_factory(Arc::new(LibopusFactory))
            .clock(Arc::clone(&self.clock))
            .build();
        neteq.register_payload_type(
            i32::from(NETEQ_OPUS_PT),
            &SdpAudioFormat::new("opus", SAMPLE_RATE as i32, 2),
        );
        tracing::info!(
            ssrc,
            streams = self.streams.len() + 1,
            "remote audio stream started"
        );
        self.streams.push(RemoteStream {
            ssrc,
            neteq,
            normalizer: LoudnessNormalizer::speech(),
            last_packet_tick: self.tick,
        });
        self.streams.len() - 1
    }

    /// Produce the next 10 ms of mixed remote audio. Call once per 10 ms the output device plays.
    pub(crate) fn pull(&mut self, out: &mut [f32; FRAME_10MS]) {
        self.tick += 1;
        out.fill(0.0);
        for stream in &mut self.streams {
            if stream.neteq.get_audio(&mut self.frame, None).is_err() {
                self.totals.errors += 1;
                continue;
            }
            if self.frame.muted() {
                continue;
            }
            for (dst, &src) in self.scratch.iter_mut().zip(self.frame.data()) {
                *dst = f32::from(src) / 32_768.0;
            }
            // Per-stream speech leveling: lifts quiet mics and tames hot ones before the mix.
            stream.normalizer.process_frame(&mut self.scratch);
            for (acc, &sample) in out.iter_mut().zip(&self.scratch) {
                *acc = (*acc + sample).clamp(-1.0, 1.0);
            }
        }
    }

    /// Cumulative counters and current delay gauges (`underruns` belongs to the [`PlayoutRing`]).
    pub(crate) fn stats(&self) -> AudioRxSnapshot {
        let mut stats = self.totals;
        for stream in &self.streams {
            stream.add_stats(&mut stats);
            let target = stream.neteq.target_delay_ms().max(0) as u32;
            let current = stream.neteq.filtered_current_delay_ms().max(0) as u32;
            stats.target_delay_ms = stats.target_delay_ms.max(target);
            stats.current_delay_ms = stats.current_delay_ms.max(current);
        }
        stats.streams = self.streams.len() as u32;
        stats
    }
}

// ---------------------------------------------------------------------------
// Playout thread -> render callback
// ---------------------------------------------------------------------------

/// Lock-free hand-off of mixed 48 kHz mono audio from the playout thread to the real-time render
/// callback. The callback's consumption is what makes the playout thread pull more, so the device
/// clock sets the pull rate.
pub(crate) struct PlayoutRing {
    ring: SpscRing<f32>,
    wake: WorkerWake,
    /// Slowly decaying maximum of the render callback size in samples.
    callback_samples: AtomicUsize,
    last_sample_bits: AtomicU32,
    underruns: AtomicU64,
}

impl PlayoutRing {
    pub(crate) fn new() -> Self {
        Self {
            ring: SpscRing::new(RING_CAPACITY),
            wake: WorkerWake::default(),
            // Assume 20 ms callbacks until the device shows its real size.
            callback_samples: AtomicUsize::new(FRAME_10MS * 2),
            last_sample_bits: AtomicU32::new(0.0f32.to_bits()),
            underruns: AtomicU64::new(0),
        }
    }

    /// Fill level the playout thread restores: one device callback plus 10 ms for its own
    /// wake-up latency. That is 20-30 ms of extra latency at the usual 10 ms callbacks.
    fn target(&self) -> usize {
        (self.callback_samples.load(Ordering::Relaxed) + FRAME_10MS)
            .min(self.ring.capacity() - FRAME_10MS)
    }

    fn wants_frame(&self) -> bool {
        self.ring.len() < self.target()
    }

    /// Real-time render callback: fill `out` without locking or allocating.
    pub(crate) fn render(&self, out: &mut [f32]) {
        let prev = self.callback_samples.load(Ordering::Relaxed);
        self.callback_samples
            .store(out.len().max(prev - prev / 64), Ordering::Relaxed);

        let got = self.ring.pop_slice(out);
        let mut last = match got {
            0 => f32::from_bits(self.last_sample_bits.load(Ordering::Relaxed)),
            n => out[n - 1],
        };
        if got < out.len() {
            // The playout thread fell behind: decay from the last sample instead of stepping to
            // zero, which would click.
            self.underruns.fetch_add(1, Ordering::Relaxed);
            for dst in &mut out[got..] {
                last *= 0.85;
                if last.abs() < 1e-5 {
                    last = 0.0;
                }
                *dst = last;
            }
        }
        self.last_sample_bits
            .store(last.to_bits(), Ordering::Relaxed);
        self.wake.wake_if_parked();
    }

    pub(crate) fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }

    /// Playout thread: make this thread the one `render` wakes.
    pub(crate) fn register_worker(&self) {
        self.wake.register();
    }

    /// Playout thread: sleep until the device has consumed audio. The timeout bounds how long a
    /// stalled device delays packet insertion and shutdown.
    pub(crate) fn wait_for_demand(&self, timeout: Duration, keep_waiting: impl Fn() -> bool) {
        self.wake
            .park_while(timeout, || !self.wants_frame() && keep_waiting());
    }
}

/// One playout-thread step: give newly arrived packets to their jitter buffers, then top the ring
/// up with as many 10 ms frames as the device has consumed. Returns the frames produced.
pub(crate) fn service(
    inbound: &mut mpsc::UnboundedReceiver<InboundAudioPacket>,
    mixer: &mut RemoteAudioMixer,
    ring: &PlayoutRing,
    mut on_frame: impl FnMut(&[f32; FRAME_10MS]),
) -> u32 {
    while let Ok(pkt) = inbound.try_recv() {
        mixer.insert(&pkt);
    }
    let mut frame = [0.0f32; FRAME_10MS];
    let mut produced = 0;
    while ring.wants_frame() {
        mixer.pull(&mut frame);
        on_frame(&frame);
        ring.ring.push_slice(&frame);
        produced += 1;
    }
    produced
}
