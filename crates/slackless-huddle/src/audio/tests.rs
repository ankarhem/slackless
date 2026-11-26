use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use bytes::Bytes;
use opus::{Application, Bitrate, Channels, Decoder, Encoder};
use tokio::sync::mpsc;

use super::dsp::{build_sonora, downmix_to_mono_f32, soft_limit_sample};
use super::playout::{MAX_JITTER_DELAY_MS, RemoteAudioMixer, service};
use super::*;
use crate::HuddleAudioConfig;
use crate::rtc::InboundAudioPacket;

#[test]
fn loudness_normalizer_lifts_quiet_tames_loud_and_ignores_silence() {
    let mut quiet_norm = LoudnessNormalizer::speech();
    let mut quiet_frame = vec![0.0f32; FRAME_20MS];
    // Feed 1.5s of quiet speech (~0.025 peak -> ~0.0177 RMS).
    for f in 0..75 {
        for (i, s) in quiet_frame.iter_mut().enumerate() {
            let t = (f * FRAME_20MS + i) as f32 / SAMPLE_RATE as f32;
            *s = (2.0 * std::f32::consts::PI * 300.0 * t).sin() * 0.025;
        }
        quiet_norm.process_frame(&mut quiet_frame);
    }
    assert!(
        quiet_norm.gain() > 2.2,
        "expected quiet voice to be boosted, got gain={}",
        quiet_norm.gain()
    );

    let mut loud_norm = LoudnessNormalizer::speech();
    let mut loud_frame = vec![0.0f32; FRAME_20MS];
    // Feed hot mic (~0.85 peak -> ~0.60 RMS): should tame quickly with zero hard clipping.
    for f in 0..25 {
        for (i, s) in loud_frame.iter_mut().enumerate() {
            let t = (f * FRAME_20MS + i) as f32 / SAMPLE_RATE as f32;
            *s = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.85;
        }
        loud_norm.process_frame(&mut loud_frame);
        assert!(
            loud_frame.iter().all(|s| s.abs() <= 0.985),
            "soft limiter must prevent clipping"
        );
    }
    assert!(
        loud_norm.gain() < 0.45,
        "expected hot voice to be attenuated, got gain={}",
        loud_norm.gain()
    );

    // Background noise below gate (-50 dBFS) must NOT be boosted.
    let mut noise_norm = LoudnessNormalizer::speech();
    let mut noise = vec![0.002f32; FRAME_20MS];
    for _ in 0..50 {
        noise.fill(0.002);
        noise_norm.process_frame(&mut noise);
    }
    assert!(
        (noise_norm.gain() - 1.0).abs() < 0.05,
        "noise below gate must remain near unity, got {}",
        noise_norm.gain()
    );

    // File RMS normalization scales louder tracks down and quieter tracks up cleanly.
    let loud_vol = normalized_gain_for_rms(0.30, 0.063, 0.06, 0.72);
    let quiet_vol = normalized_gain_for_rms(0.10, 0.063, 0.06, 0.72);
    assert!((loud_vol - 0.21).abs() < 0.01);
    assert!((quiet_vol - 0.63).abs() < 0.01);
}

#[test]
fn synthetic_sonora_and_opus_round_trip_preserves_signal() {
    let mut apm = build_sonora(true, true, true);
    let mut enc = Encoder::new(SAMPLE_RATE, Channels::Mono, Application::Voip).unwrap();
    let mut dec = Decoder::new(SAMPLE_RATE, Channels::Mono).unwrap();

    let mut sine = Vec::with_capacity(FRAME_20MS);
    for i in 0..FRAME_20MS {
        let t = i as f32 / SAMPLE_RATE as f32;
        sine.push((2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.25);
    }
    let mut processed = Vec::with_capacity(FRAME_20MS);
    for chunk in sine.as_chunks::<FRAME_10MS>().0 {
        let mut out = [0.0f32; FRAME_10MS];
        apm.process_capture_f32(&[chunk.as_slice()], &mut [&mut out[..]])
            .unwrap();
        processed.extend_from_slice(&out);
    }
    let mut encoded = vec![0u8; 4000];
    let n = enc.encode_float(&processed, &mut encoded).unwrap();
    assert!(n > 0);
    let mut decoded = vec![0.0f32; FRAME_20MS];
    let samples = dec
        .decode_float(&encoded[..n], &mut decoded, false)
        .unwrap();
    assert_eq!(samples, FRAME_20MS);
    assert!(compute_rms_mono(&decoded) > 0.01);
}

/// Amplitude of `freq` Hz in `x` (Goertzel), exact when the window holds a whole number of
/// periods of it.
fn tone_amplitude(x: &[f32], freq: f32, rate: u32) -> f32 {
    let w = std::f32::consts::TAU * freq / rate as f32;
    let coeff = 2.0 * f64::from(w.cos());
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &v in x {
        let s0 = f64::from(v) + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    ((s1 * s1 + s2 * s2 - s1 * s2 * coeff).sqrt() * 2.0 / x.len() as f64) as f32
}

#[test]
fn rate_converter_is_band_limited_and_keeps_the_rate_ratio() {
    // (source, target, tone to keep, tone above the target Nyquist to reject).
    // A 44.1 kHz source has nothing above its own Nyquist to reject.
    const PAIRS: [(u32, u32, f32, f32); 3] = [
        (44_100, 48_000, 1_000.0, 0.0),
        (48_000, 16_000, 1_000.0, 12_000.0),
        (48_000, 44_100, 1_000.0, 23_000.0),
    ];
    // The block sizes the real-time and the offline paths use.
    const BLOCK_MS: [usize; 2] = [10, 100];
    const AMPLITUDE: f32 = 0.5;
    // 0.5 dB of amplitude, and the 40 dB rejection floor.
    const PASSBAND_TOLERANCE: f32 = 0.0593;
    const STOPBAND_FLOOR: f32 = 0.01;

    for (src, dst, kept, rejected) in PAIRS {
        for block_ms in BLOCK_MS {
            let mut conv = RateConverter::new(src, dst, src as usize * block_ms / 1_000).unwrap();
            let input: Vec<f32> = (0..src as usize)
                .map(|i| {
                    let t = i as f32 / src as f32;
                    let mut v = (std::f32::consts::TAU * kept * t).sin();
                    if rejected > 0.0 {
                        v += (std::f32::consts::TAU * rejected * t).sin();
                    }
                    v * AMPLITUDE
                })
                .collect();
            let mut out = Vec::new();
            conv.process(&input, &mut out);
            // One second in: the rate ratio to within one block. Measure 400 ms from one block
            // in (the first block is the filter's ramp-up, the last its tail); 10 ms multiples
            // put both tones on-bin for the measurement.
            let ctx = format!("{src} -> {dst} Hz, {block_ms} ms blocks");
            assert!(
                (out.len() as isize - dst as isize).abs() <= conv.chunk_out() as isize,
                "{ctx}: {} frames out",
                out.len()
            );
            let window = &out[conv.chunk_out()..conv.chunk_out() + dst as usize * 2 / 5];
            let kept_amp = tone_amplitude(window, kept, dst);
            assert!(
                (kept_amp - AMPLITUDE).abs() <= AMPLITUDE * PASSBAND_TOLERANCE,
                "{ctx}: {kept} Hz at {kept_amp}"
            );
            if rejected > 0.0 {
                let rejected_amp = tone_amplitude(window, rejected, dst);
                assert!(
                    rejected_amp <= AMPLITUDE * STOPBAND_FLOOR,
                    "{ctx}: {rejected} Hz at {rejected_amp}"
                );
            }
        }
    }
}

#[test]
fn official_slack_huddle_sounds_and_lounge_music_resolve_to_slack_cdn() {
    assert_eq!(
        HuddleChime::Join.slack_cdn_url(),
        "https://a.slack-edge.com/bv1-13/you_joined_call_v2-7029418.mp3"
    );
    assert_eq!(
        HuddleChime::Leave.slack_cdn_url(),
        "https://a.slack-edge.com/bv1-13/you_left_call_v2-1bc1290.mp3"
    );
    assert_eq!(
        HuddleChime::PeerJoin.slack_cdn_url(),
        "https://a.slack-edge.com/bv1-13/they_joined_call_v2-570b169.mp3"
    );
    assert_eq!(
        HuddleChime::PeerLeave.slack_cdn_url(),
        "https://a.slack-edge.com/bv1-13/they_left_call_v2-95d3260.mp3"
    );
    assert_eq!(
        HuddleChime::InviteRing.slack_cdn_url(),
        "https://a.slack-edge.com/bv1-13/outgoing_ring_v2-6b3697e.mp3"
    );
    assert_eq!(
        slack_lounge_music_filename(None),
        "waiting_muzak-9187b04.mp3"
    );
    assert_eq!(
        slack_lounge_music_filename(Some("jazz")),
        "waiting_muzak-9187b04.mp3"
    );
    assert_eq!(
        slack_lounge_music_filename(Some("lofi")),
        "huddles_focus_lofihaus_music_mixdown_stereo-07cf114.mp3"
    );
}

#[test]
fn dsp_helpers_and_shared_state_edge_cases() {
    // LoudnessNormalizer: empty frame no-op & silence drift toward unity.
    let mut norm = LoudnessNormalizer::speech();
    norm.process_frame(&mut []);
    assert!((norm.gain() - 1.0).abs() < 1e-6);
    let mut loud = vec![0.8f32; FRAME_20MS];
    for _ in 0..10 {
        loud.fill(0.8);
        norm.process_frame(&mut loud);
    }
    let attenuated = norm.gain();
    assert!(attenuated < 0.5);
    let mut silence = vec![0.0f32; FRAME_20MS];
    for _ in 0..100 {
        silence.fill(0.0);
        norm.process_frame(&mut silence);
    }
    assert!(
        norm.gain() > attenuated,
        "silence must drift gain back toward 1.0"
    );

    // soft_limit_sample: linear inside knee, bounded outside, antisymmetric.
    assert_eq!(soft_limit_sample(0.0), 0.0);
    assert_eq!(soft_limit_sample(0.80), 0.80);
    assert_eq!(soft_limit_sample(-0.80), -0.80);
    assert!(soft_limit_sample(10.0) <= 0.98 && soft_limit_sample(10.0) > 0.95);
    assert!((soft_limit_sample(-10.0) + soft_limit_sample(10.0)).abs() < 1e-6);

    // normalized_gain_for_rms edge cases (NaN, Inf, 0, negative, clamp bounds).
    assert!((normalized_gain_for_rms(f32::NAN, 0.1, 0.2, 5.0) - 0.2).abs() < 1e-6);
    assert!((normalized_gain_for_rms(f32::INFINITY, 0.5, 0.2, 5.0) - 0.5).abs() < 1e-6);
    assert!((normalized_gain_for_rms(f32::NEG_INFINITY, 0.5, 0.2, 5.0) - 0.5).abs() < 1e-6);
    assert!((normalized_gain_for_rms(0.0, 0.5, 0.2, 5.0) - 0.5).abs() < 1e-6);
    assert!((normalized_gain_for_rms(-1.0, 0.5, 0.2, 5.0) - 0.5).abs() < 1e-6);
    assert!((normalized_gain_for_rms(0.001, 0.5, 0.2, 5.0) - 5.0).abs() < 1e-6);
    assert!((normalized_gain_for_rms(10.0, 0.5, 0.2, 5.0) - 0.2).abs() < 1e-6);

    // downmix_to_mono_f32.
    assert!(downmix_to_mono_f32(&[], 2).is_empty());
    assert_eq!(downmix_to_mono_f32(&[0.2, -0.4], 1), vec![0.2, -0.4]);
    assert_eq!(
        downmix_to_mono_f32(&[0.2, 0.6, -0.4, 0.4], 2),
        vec![0.4, 0.0]
    );
    assert_eq!(downmix_to_mono_f32(&[1.0, 1.0, 0.0, 0.0], 4), vec![0.5]);

    // compute_rms_mono.
    assert_eq!(compute_rms_mono(&[]), 0.0);
    assert!((compute_rms_mono(&[0.5; 10]) - 0.5).abs() < 1e-6);
    assert_eq!(compute_rms_mono(&[2.0; 10]), 1.0);

    // AudioSharedState::local_rms respects muted flag.
    let cfg = HuddleAudioConfig::default();
    let shared = AudioSharedState::from_config(&cfg);
    shared
        .local_rms_bits
        .store(0.42f32.to_bits(), Ordering::Relaxed);
    assert!((shared.local_rms() - 0.42).abs() < 1e-6);
    shared.muted.store(true, Ordering::Relaxed);
    assert_eq!(shared.local_rms(), 0.0);
}

// ---------------------------------------------------------------------------
// Remote audio receive + playout: one scenario table, one set of invariants
// ---------------------------------------------------------------------------

/// 0.2 s of silence, then four 0.9 s voiced bursts each followed by 0.6 s of silence.
const SIGNAL_FRAMES: usize = 310;
/// Frames delivered cleanly before a scenario's impairments start (NetEq's start-up second).
const WARMUP_FRAMES: usize = 50;
/// Samples the virtual output device pulls per callback (CoreAudio's usual buffer).
const DEVICE_CALLBACK: usize = 512;
/// Sample magnitude that counts as audible; the bursts peak near 0.25.
const AUDIBLE: f32 = 0.02;

/// What the sender and the network do to one run of the receive + playout core.
#[derive(Clone, Copy)]
struct Scenario {
    name: &'static str,
    /// Opus DTX as a WebRTC sender does it: silence is not sent, except the first DTX frame.
    dtx: bool,
    /// Padding-only packets after every frame (alternating n and n-1), as Chime sends them.
    padding: u16,
    /// Independent packet loss in percent.
    loss_pct: u64,
    /// (every, len): len consecutive frames are lost every "every" frames.
    burst: (usize, usize),
    /// Every n-th packet arrives after its successor.
    reorder_every: usize,
    /// Every n-th packet arrives twice.
    duplicate_every: usize,
    /// (every, ms): every "every" frames the network stalls for ms, then delivers the backlog.
    spike: (usize, u64),
    /// Sender clock error relative to the output device, in parts per million.
    sender_ppm: i64,
    /// How many times the 6.2 s signal is sent back to back.
    loops: usize,
    first_seq: u16,
    first_timestamp: u32,
}

const CLEAN: Scenario = Scenario {
    name: "clean",
    dtx: false,
    padding: 0,
    loss_pct: 0,
    burst: (0, 0),
    reorder_every: 0,
    duplicate_every: 0,
    spike: (0, 0),
    sender_ppm: 0,
    loops: 1,
    first_seq: 1_000,
    first_timestamp: 90_000,
};

/// Voiced test signal: harmonic bursts with a gliding pitch, separated by digital silence.
fn voiced_test_signal() -> Vec<f32> {
    let mut phase = 0.0f32;
    (0..SIGNAL_FRAMES * FRAME_20MS)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            let in_cycle = (t - 0.2).rem_euclid(1.5);
            if t < 0.2 || in_cycle >= 0.9 {
                phase = 0.0;
                return 0.0;
            }
            let pitch_hz = 110.0 + 80.0 * in_cycle / 0.9;
            phase += std::f32::consts::TAU * pitch_hz / SAMPLE_RATE as f32;
            let fade = (in_cycle / 0.02).min((0.9 - in_cycle) / 0.02).min(1.0);
            let voice: f32 = (1..=8).map(|k| (phase * k as f32).sin() / k as f32).sum();
            0.12 * fade * voice
        })
        .collect()
}

/// The signal as 20 ms Opus frames on the wire; None is a frame DTX suppressed.
fn encode_frames(pcm: &[f32], dtx: bool) -> Vec<Option<Bytes>> {
    let mut enc = Encoder::new(SAMPLE_RATE, Channels::Mono, Application::Voip).unwrap();
    enc.set_bitrate(Bitrate::Bits(32_000)).unwrap();
    enc.set_signal(opus::Signal::Voice).unwrap();
    enc.set_inband_fec(true).unwrap();
    enc.set_packet_loss_perc(10).unwrap();
    enc.set_dtx(dtx).unwrap();
    let mut buf = [0u8; 1_500];
    let mut in_dtx = false;
    pcm.as_chunks::<FRAME_20MS>()
        .0
        .iter()
        .map(|frame| {
            let n = enc.encode_float(frame, &mut buf).unwrap();
            let was_dtx = std::mem::replace(&mut in_dtx, dtx && n <= 2);
            (!(in_dtx && was_dtx)).then(|| Bytes::copy_from_slice(&buf[..n]))
        })
        .collect()
}

/// RTP packets with their arrival time in microseconds, in arrival order, plus the media the
/// scenario makes unavailable in time (lost or stalled), in samples.
fn packetize(
    frames: &[Option<Bytes>],
    sc: &Scenario,
    epoch: Instant,
) -> (Vec<(u64, InboundAudioPacket)>, u64) {
    let mut rng = 0x9E37_79B9_7F4A_7C15u64;
    let mut random = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let mut wire = Vec::new();
    let mut unavailable = 0u64;
    let mut seq = sc.first_seq;
    let mut stalled_until = 0u64;
    for (i, payload) in frames
        .iter()
        .cycle()
        .take(frames.len() * sc.loops)
        .enumerate()
    {
        let Some(payload) = payload else { continue };
        let impaired = i >= WARMUP_FRAMES;
        let hits = |every: usize| impaired && every > 0 && i % every == 0;
        let timestamp = sc.first_timestamp.wrapping_add((i * FRAME_20MS) as u32);
        let mut emit = |at: u64, seq: u16, payload: Bytes| {
            let arrival = epoch + Duration::from_micros(at);
            wire.push((
                at,
                InboundAudioPacket {
                    ssrc: 0x00C0_FFEE,
                    seq,
                    timestamp,
                    marker: false,
                    payload,
                    arrival,
                },
            ));
        };

        // A frame leaves when its last sample was captured, by the sender's own clock, and
        // takes 30-34 ms through the network.
        let sent = (i as u64 + 1) * 20_000 * 1_000_000 / (1_000_000 + sc.sender_ppm) as u64;
        let mut at = sent + 30_000 + random() % 4_000;
        if hits(sc.spike.0) {
            stalled_until = at + sc.spike.1 * 1_000;
            unavailable += sc.spike.1 * 48;
        }
        at = at.max(stalled_until);
        if hits(sc.reorder_every) {
            at += 26_000;
        }
        let lost = impaired
            && (random() % 100 < sc.loss_pct || (sc.burst.0 > 0 && i % sc.burst.0 < sc.burst.1));
        if lost {
            unavailable += FRAME_20MS as u64;
        } else {
            emit(at, seq, payload.clone());
            if hits(sc.duplicate_every) {
                emit(at + 2_000, seq, payload.clone());
            }
        }
        seq = seq.wrapping_add(1);
        for n in 0..sc.padding.saturating_sub((i % 2) as u16) {
            emit(at + 200 * (u64::from(n) + 1), seq, Bytes::new());
            seq = seq.wrapping_add(1);
        }
    }
    wire.sort_by_key(|(at, _)| *at);
    (wire, unavailable)
}

struct Played {
    pcm: Vec<f32>,
    /// Counters when the last packet had arrived (afterwards NetEq conceals the ended stream).
    stats: AudioRxSnapshot,
    max_delay_ms: u32,
    /// (media, padding) packets put on the wire.
    sent: (u64, u64),
    unavailable: u64,
}

/// Run one scenario through the production path (packet channel -> service -> NetEq mixer ->
/// PlayoutRing -> render) on a virtual clock: a device callback every 512 samples, each followed
/// by the playout-thread step it would wake.
fn play(sc: &Scenario, frames: &[Option<Bytes>]) -> Played {
    let epoch = Instant::now();
    let clock = neteq::api::SimulatedClock::new(0);
    let mut mixer = RemoteAudioMixer::new(epoch, Arc::new(clock.clone()));
    let ring = PlayoutRing::new();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (wire, unavailable) = packetize(frames, sc, epoch);
    let padding = wire.iter().filter(|(_, p)| p.payload.is_empty()).count() as u64;
    let sent = (wire.len() as u64 - padding, padding);
    let mut wire = wire.into_iter().peekable();

    let samples = frames.len() * sc.loops * FRAME_20MS + SAMPLE_RATE as usize;
    let mut pcm = Vec::with_capacity(samples);
    let mut callback = [0.0f32; DEVICE_CALLBACK];
    let mut max_delay_ms = 0;
    let mut stats = None;
    service(&mut rx, &mut mixer, &ring, |_| {});
    while pcm.len() < samples {
        let now = pcm.len() as u64 * 1_000_000 / u64::from(SAMPLE_RATE);
        ring.render(&mut callback);
        pcm.extend_from_slice(&callback);
        while let Some((_, pkt)) = wire.next_if(|(at, _)| *at <= now) {
            tx.send(pkt).unwrap();
        }
        clock.set_time_us(now as i64);
        service(&mut rx, &mut mixer, &ring, |_| {});
        if stats.is_none() {
            max_delay_ms = max_delay_ms.max(mixer.stats().current_delay_ms);
            if wire.peek().is_none() {
                stats = Some(AudioRxSnapshot {
                    underruns: ring.underruns(),
                    ..mixer.stats()
                });
            }
        }
    }
    Played {
        pcm,
        stats: stats.unwrap(),
        max_delay_ms,
        sent,
        unavailable,
    }
}

/// Mouth-to-ear latency of every voiced burst in ms: from the moment its first sample left the
/// sender to the first audible output sample at or after that moment.
fn burst_latencies_ms(pcm: &[f32], sc: &Scenario) -> Vec<f64> {
    (0..sc.loops * 4)
        .map(|burst| {
            let onset = (burst / 4) * SIGNAL_FRAMES * FRAME_20MS
                + (SAMPLE_RATE as usize / 10) * (2 + 15 * (burst % 4));
            let sent = onset as f64 * 1e6 / (1e6 + sc.sender_ppm as f64);
            let heard = pcm[sent as usize..]
                .iter()
                .position(|s| s.abs() > AUDIBLE)
                .unwrap();
            ((sent as usize + heard) as f64 - sent) / 48.0
        })
        .collect()
}

#[test]
fn remote_audio_playout_holds_its_invariants_across_network_scenarios() {
    #[rustfmt::skip]
    let scenarios = [
        CLEAN,
        Scenario { name: "chime padding", padding: 4, ..CLEAN },
        Scenario { name: "dtx gaps", dtx: true, ..CLEAN },
        Scenario { name: "5% random loss", loss_pct: 5, ..CLEAN },
        Scenario { name: "burst loss", burst: (60, 5), ..CLEAN },
        Scenario { name: "reorder", reorder_every: 7, ..CLEAN },
        Scenario { name: "duplicates", duplicate_every: 3, ..CLEAN },
        Scenario { name: "jitter spikes", spike: (55, 120), ..CLEAN },
        Scenario { name: "chime padding + 5% loss", padding: 4, loss_pct: 5, ..CLEAN },
        // 310 s each: 62 ms of drift that only time stretching can absorb.
        Scenario { name: "sender +200 ppm", sender_ppm: 200, loops: 50, ..CLEAN },
        Scenario { name: "sender -200 ppm", sender_ppm: -200, loops: 50, ..CLEAN },
        Scenario { name: "seq + timestamp wraparound", first_seq: 65_400, first_timestamp: u32::MAX - 96_000, ..CLEAN },
    ];
    let max_step = |pcm: &[f32]| {
        pcm.windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max)
    };
    let signal = voiced_test_signal();
    let encoded = [encode_frames(&signal, false), encode_frames(&signal, true)];

    for sc in scenarios {
        let played = play(&sc, &encoded[usize::from(sc.dtx)]);
        let stats = played.stats;
        let latency = burst_latencies_ms(&played.pcm, &sc);
        let (fastest, slowest) = latency[1..]
            .iter()
            .fold((f64::MAX, 0.0f64), |(lo, hi), &l| (lo.min(l), hi.max(l)));
        let step = max_step(&played.pcm);
        // What the scenario itself excuses: media that was lost or stalled, capped at the
        // jitter buffer's window.
        let excused_ms = (played.unavailable as f64 / 48.0).min(f64::from(MAX_JITTER_DELAY_MS));
        let ctx = format!(
            "[{}] {stats:?} latency {:.0}..{slowest:.0} ms, max step {step:.3}",
            sc.name,
            fastest.min(latency[0])
        );

        // 1. Every packet is either media in a jitter buffer or skipped padding: padding is
        //    never decoded and never stands in for a lost packet.
        assert_eq!((stats.packets, stats.padding), played.sent, "{ctx}");
        assert_eq!(stats.errors, 0, "{ctx}");
        // 2. Concealment only covers media the network really withheld: none at all for clean,
        //    padded, DTX, reordered and duplicated streams.
        assert!(
            stats.concealed_samples <= played.unavailable * 5 / 4 + FRAME_10MS as u64,
            "{ctx}"
        );
        // 3. Output tracks media time: once NetEq has settled, bursts are heard as far apart
        //    as they were sent (no creeping delay, no skipped or repeated stretches).
        assert!(slowest - fastest <= 40.0 + excused_ms, "{ctx}");
        // 4. Latency stays in the window: never faster than packetization + network allow,
        //    never beyond the fixed path plus what the scenario excuses, and NetEq's own delay
        //    stays under its configured maximum.
        assert!(
            latency
                .iter()
                .all(|&l| (50.0..=160.0 + excused_ms).contains(&l)),
            "{ctx}"
        );
        assert!(
            played.max_delay_ms.max(stats.target_delay_ms) <= MAX_JITTER_DELAY_MS as u32,
            "{ctx}"
        );
        // 5. The device is never starved and never hears a discontinuity.
        assert_eq!(stats.underruns, 0, "{ctx}");
        assert!(step <= 3.0 * max_step(&signal), "{ctx}");
    }
}
