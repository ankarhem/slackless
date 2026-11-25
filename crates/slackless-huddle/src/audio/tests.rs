use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use opus::{Application, Bitrate, Channels, Decoder, Encoder};
use tokio::sync::mpsc;

use super::dsp::{build_sonora, downmix_to_mono_f32, soft_limit_sample};
use super::engine::run_inbound_audio_rx_loop;
use super::jitter::{PLAYBACK_MAX_SAMPLES, PLAYBACK_PRIME_SAMPLES, PLAYBACK_TRIM_TARGET};
use super::ring::SpscRing;
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

#[test]
fn opus_plc_conceals_lost_packet_without_silence_click() {
    let mut enc = Encoder::new(SAMPLE_RATE, Channels::Mono, Application::Voip).unwrap();
    let mut dec = Decoder::new(SAMPLE_RATE, Channels::Mono).unwrap();

    let sine: Vec<f32> = (0..FRAME_20MS)
        .map(|i| (2.0 * std::f32::consts::PI * 330.0 * (i as f32 / SAMPLE_RATE as f32)).sin() * 0.3)
        .collect();
    let mut encoded = vec![0u8; 4000];
    let mut decoded = vec![0.0f32; FRAME_20MS * 2];

    // Prime the decoder with two normal frames so PLC has history to extrapolate.
    for _ in 0..2 {
        let n = enc.encode_float(&sine, &mut encoded).unwrap();
        let got = decode_opus_or_plc(&mut dec, &encoded[..n], &mut decoded).unwrap();
        assert_eq!(got, FRAME_20MS);
    }

    // Empty slice invokes Opus PLC for a missing 20ms RTP frame.
    let plc_samples = decode_opus_or_plc(&mut dec, &[], &mut decoded).unwrap();
    assert_eq!(plc_samples, FRAME_20MS);
    assert!(compute_rms_mono(&decoded[..plc_samples]) > 0.005);
}

#[test]
fn linear_resampler_scales_length_accurately() {
    let input = vec![0.5f32; 441];
    let out = resample_linear(&input, 44_100, 48_000);
    assert_eq!(out.len(), 480);
    assert!((out[240] - 0.5).abs() < 1e-4);
}

#[test]
fn playback_buffer_prebuffers_and_fades_smoothly_on_underrun() {
    let pb = PlaybackBuffer::new();
    let mut out = vec![1.0f32; 512];

    // Below 60ms prime threshold: outputs silence so 512-sample CoreAudio pulls never sputter.
    pb.push(&vec![0.4f32; FRAME_20MS]);
    pb.pull_into(&mut out);
    assert!(out.iter().all(|&s| s == 0.0));

    // Pushing 2 more 20ms frames reaches 60ms -> primed and plays cleanly!
    pb.push(&vec![0.4f32; FRAME_20MS]);
    pb.push(&vec![0.4f32; FRAME_20MS]);
    pb.pull_into(&mut out);
    assert!(out.iter().all(|&s| (s - 0.4).abs() < 1e-5));

    // Draining past available samples fades smoothly toward zero (no hard step click) and unprimes.
    let mut big = vec![0.0f32; FRAME_20MS * 4];
    pb.pull_into(&mut big);
    let tail_start = PLAYBACK_PRIME_SAMPLES - 512;
    assert!(big[tail_start] > 0.2 && big[tail_start] < 0.4);
    assert!(big[tail_start + 64].abs() < 1e-4);
    assert!(!pb.is_primed());
}

#[test]
fn jitter_buffer_reorders_out_of_order_packets_and_recovers_loss_with_fec() {
    let mut jb = AudioJitterBuffer::new();
    let t0 = Instant::now();

    // Out-of-order UDP delivery: 100, 102, 101 arrives 5ms later -> played strictly 100, 101, 102.
    jb.push(100, Bytes::from_static(b"p100"), t0);
    assert_eq!(
        jb.pop_ready(t0),
        Some(JitterStep::Packet(Bytes::from_static(b"p100")))
    );

    jb.push(
        102,
        Bytes::from_static(b"p102"),
        t0 + Duration::from_millis(20),
    );
    // Only 1 newer packet and <35ms waited -> holds briefly for reordered 101.
    assert_eq!(jb.pop_ready(t0 + Duration::from_millis(25)), None);

    jb.push(
        101,
        Bytes::from_static(b"p101"),
        t0 + Duration::from_millis(26),
    );
    assert_eq!(
        jb.pop_ready(t0 + Duration::from_millis(26)),
        Some(JitterStep::Packet(Bytes::from_static(b"p101")))
    );
    assert_eq!(
        jb.pop_ready(t0 + Duration::from_millis(26)),
        Some(JitterStep::Packet(Bytes::from_static(b"p102")))
    );

    // Genuine single-packet loss: 103 never arrives, 104 arrives -> after 35ms triggers Opus FEC from 104!
    jb.push(
        104,
        Bytes::from_static(b"p104"),
        t0 + Duration::from_millis(80),
    );
    assert_eq!(
        jb.pop_ready(t0 + Duration::from_millis(120)),
        Some(JitterStep::FecFromNext(Bytes::from_static(b"p104")))
    );
    assert_eq!(
        jb.pop_ready(t0 + Duration::from_millis(120)),
        Some(JitterStep::Packet(Bytes::from_static(b"p104")))
    );
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
fn jitter_buffer_wraparound_stale_multi_loss_fast_trigger_dtx_resync_overflow_and_rearm() {
    let t0 = Instant::now();

    // 1. 16-bit seq wraparound (65534..=1) + out-of-order across wrap.
    let mut jb = AudioJitterBuffer::new();
    jb.push(65534, Bytes::from_static(b"a"), t0);
    jb.push(0, Bytes::from_static(b"c"), t0 + Duration::from_millis(5));
    jb.push(
        65535,
        Bytes::from_static(b"b"),
        t0 + Duration::from_millis(8),
    );
    jb.push(1, Bytes::from_static(b"d"), t0 + Duration::from_millis(12));
    for expected in [b"a", b"b", b"c", b"d"] {
        assert_eq!(
            jb.pop_ready(t0 + Duration::from_millis(12)),
            Some(JitterStep::Packet(Bytes::from_static(expected)))
        );
    }

    // 2. Stale packets (already played) are ignored; duplicate of queued seq doesn't corrupt.
    jb.push(
        65535,
        Bytes::from_static(b"stale"),
        t0 + Duration::from_millis(20),
    );
    assert_eq!(jb.pop_ready(t0 + Duration::from_millis(20)), None);
    jb.push(2, Bytes::from_static(b"v1"), t0 + Duration::from_millis(20));
    jb.push(2, Bytes::from_static(b"v2"), t0 + Duration::from_millis(21));
    assert_eq!(
        jb.pop_ready(t0 + Duration::from_millis(21)),
        Some(JitterStep::Packet(Bytes::from_static(b"v2")))
    );

    // 3. Multi-packet loss (3 & 4 lost, 5 arrives): after 35ms yields Plc then FecFromNext(5) then Packet(5).
    jb.push(5, Bytes::from_static(b"p5"), t0 + Duration::from_millis(40));
    assert_eq!(jb.pop_ready(t0 + Duration::from_millis(60)), None);
    assert_eq!(
        jb.pop_ready(t0 + Duration::from_millis(80)),
        Some(JitterStep::Plc)
    );
    assert_eq!(
        jb.pop_ready(t0 + Duration::from_millis(80)),
        Some(JitterStep::FecFromNext(Bytes::from_static(b"p5")))
    );
    assert_eq!(
        jb.pop_ready(t0 + Duration::from_millis(80)),
        Some(JitterStep::Packet(Bytes::from_static(b"p5")))
    );

    // 4. `enough_newer` fast-trigger: 2 newer packets within gap <= 4 trigger immediately without 35ms wait.
    jb.push(
        7,
        Bytes::from_static(b"p7"),
        t0 + Duration::from_millis(100),
    );
    jb.push(
        8,
        Bytes::from_static(b"p8"),
        t0 + Duration::from_millis(101),
    );
    assert_eq!(
        jb.pop_ready(t0 + Duration::from_millis(101)),
        Some(JitterStep::FecFromNext(Bytes::from_static(b"p7")))
    );
    assert_eq!(
        jb.pop_ready(t0 + Duration::from_millis(101)),
        Some(JitterStep::Packet(Bytes::from_static(b"p7")))
    );
    assert_eq!(
        jb.pop_ready(t0 + Duration::from_millis(101)),
        Some(JitterStep::Packet(Bytes::from_static(b"p8")))
    );

    // 5. DTX jump (gap > 4 and <= 20): waits 35ms then jumps straight to first available packet.
    jb.push(
        16,
        Bytes::from_static(b"p16"),
        t0 + Duration::from_millis(200),
    );
    assert_eq!(jb.pop_ready(t0 + Duration::from_millis(220)), None);
    assert_eq!(
        jb.pop_ready(t0 + Duration::from_millis(240)),
        Some(JitterStep::Packet(Bytes::from_static(b"p16")))
    );

    // 6. Large jump resync (> 20): clears queue and plays immediately.
    jb.push(
        18,
        Bytes::from_static(b"old"),
        t0 + Duration::from_millis(300),
    );
    jb.push(
        50,
        Bytes::from_static(b"resync"),
        t0 + Duration::from_millis(305),
    );
    assert_eq!(
        jb.pop_ready(t0 + Duration::from_millis(305)),
        Some(JitterStep::Packet(Bytes::from_static(b"resync")))
    );

    // 7. 24-packet overflow cap (exercised directly when queue exceeds 24 entries) + 21-packet resync boundary.
    let mut jb_cap = AudioJitterBuffer::new();
    for seq in 0u16..=20 {
        jb_cap.push(seq, Bytes::from(vec![seq as u8]), t0);
    }
    assert_eq!(jb_cap.queue.len(), 21);
    assert_eq!(jb_cap.next_seq, Some(0));
    // Pre-fill extra synthetic entries below next_seq + 20 to verify the >24 cap trims oldest.
    for extra in 21i64..26 {
        jb_cap.queue.insert(extra, Bytes::from(vec![extra as u8]));
    }
    jb_cap.push(20, Bytes::from(vec![20u8]), t0);
    assert_eq!(jb_cap.queue.len(), 24);
    assert_eq!(jb_cap.next_seq, Some(2));
    assert_eq!(
        jb_cap.pop_ready(t0),
        Some(JitterStep::Packet(Bytes::from(vec![2u8])))
    );

    // 8. `missing_since` re-arming when popping `next` exposes a new gap.
    let mut jb_rearm = AudioJitterBuffer::new();
    jb_rearm.push(10, Bytes::from_static(b"p10"), t0);
    jb_rearm.push(12, Bytes::from_static(b"p12"), t0);
    // Popping 10 at t0 + 100ms re-arms missing_since to t0 + 100ms for missing 11.
    assert_eq!(
        jb_rearm.pop_ready(t0 + Duration::from_millis(100)),
        Some(JitterStep::Packet(Bytes::from_static(b"p10")))
    );
    // At t0 + 120ms (only 20ms since re-arm), must still wait for 11.
    assert_eq!(jb_rearm.pop_ready(t0 + Duration::from_millis(120)), None);
    // At t0 + 140ms (40ms since re-arm), triggers FEC from 12.
    assert_eq!(
        jb_rearm.pop_ready(t0 + Duration::from_millis(140)),
        Some(JitterStep::FecFromNext(Bytes::from_static(b"p12")))
    );
}

#[test]
fn playback_buffer_overflow_crossfade_exact_drain_clamping_reprime_and_snap() {
    let pb = PlaybackBuffer::new();

    // Exact drain stays primed!
    pb.push(&vec![0.5f32; PLAYBACK_PRIME_SAMPLES]);
    assert!(pb.is_primed());
    let mut exact = vec![0.0f32; PLAYBACK_PRIME_SAMPLES];
    pb.pull_into(&mut exact);
    assert!(
        pb.is_primed(),
        "exact drain without shortage must stay primed"
    );
    assert!(exact.iter().all(|&s| (s - 0.5).abs() < 1e-6));

    // Clamping out-of-range samples [-1.0, 1.0].
    pb.push(&[2.5, -3.0, 0.25, -0.75]);
    let mut clamped = vec![0.0f32; 4];
    pb.pull_into(&mut clamped);
    assert_eq!(clamped, vec![1.0, -1.0, 0.25, -0.75]);

    // Re-priming cycle: trigger underrun -> unprimed -> fade snaps to exact 0.0.
    let mut fade = vec![99.0f32; 128];
    pb.pull_into(&mut fade);
    assert!(!pb.is_primed());
    assert_eq!(
        *fade.last().unwrap(),
        0.0,
        "fade_to_silence must snap to 0.0"
    );
    pb.push(&vec![0.3f32; FRAME_20MS * 2]);
    assert!(!pb.is_primed());
    pb.push(&vec![0.3f32; FRAME_20MS]);
    assert!(pb.is_primed());

    // Overflow trim + 32-sample crossfade splice (consumer-side lock-free trim).
    let pb_ov = PlaybackBuffer::new();
    // Fill first portion with -0.6 and later portion with +0.6 up to PLAYBACK_MAX_SAMPLES.
    pb_ov.push(&vec![-0.6f32; PLAYBACK_MAX_SAMPLES - PLAYBACK_TRIM_TARGET]);
    pb_ov.push(&vec![0.6f32; PLAYBACK_TRIM_TARGET]);
    assert_eq!(pb_ov.len(), PLAYBACK_MAX_SAMPLES);
    // Pushing 1 more frame exceeds PLAYBACK_MAX_SAMPLES -> consumer trims to PLAYBACK_TRIM_TARGET
    // and crossfades the first 32 pulled samples smoothly from -0.6 toward +0.6.
    pb_ov.push(&vec![0.6f32; FRAME_20MS]);
    let mut splice = vec![0.0f32; 40];
    pb_ov.pull_into(&mut splice);
    assert_eq!(pb_ov.len(), PLAYBACK_TRIM_TARGET - 40);
    assert!(splice[0] < -0.4);
    assert!(splice[15].abs() < 0.15);
    assert!((splice[31] - 0.6).abs() < 0.1);
    assert!((splice[35] - 0.6).abs() < 1e-6);
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

    // resample_linear edge cases.
    assert!(resample_linear(&[], 44_100, 48_000).is_empty());
    assert_eq!(resample_linear(&[0.1, 0.2], 48_000, 48_000), vec![0.1, 0.2]);
    let down = resample_linear(&vec![0.75f32; 480], 48_000, 16_000);
    assert_eq!(down.len(), 160);
    assert!((down[80] - 0.75).abs() < 1e-4);

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

#[test]
fn inbound_audio_rx_loop_reorders_fec_plc_primes_playback_and_gates_aec_tap() {
    let mut enc = Encoder::new(SAMPLE_RATE, Channels::Mono, Application::Voip).unwrap();
    let _ = enc.set_bitrate(Bitrate::Bits(32_000));
    let _ = enc.set_inband_fec(true);
    let _ = enc.set_packet_loss_perc(20);

    let mut packets = Vec::new();
    let mut out_buf = vec![0u8; 4000];
    for frame_ix in 0..10usize {
        let pcm: Vec<f32> = (0..FRAME_20MS)
            .map(|i| {
                let t = (frame_ix * FRAME_20MS + i) as f32 / SAMPLE_RATE as f32;
                (2.0 * std::f32::consts::PI * 350.0 * t).sin() * 0.25
            })
            .collect();
        let n = enc.encode_float(&pcm, &mut out_buf).unwrap();
        packets.push(Bytes::copy_from_slice(&out_buf[..n]));
    }

    // Case 1: needs_sw_aec_ref = true, unmuted, aec = true -> populates both PlaybackBuffer and aec_render_tap.
    let cfg = HuddleAudioConfig::default();
    let shared = AudioSharedState::from_config(&cfg);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let playback = Arc::new(PlaybackBuffer::new());
    let aec_tap = Arc::new(SpscRing::<f32>::new(FRAME_20MS * 8));
    let needs_aec = Arc::new(AtomicBool::new(true));

    let s_clone = shared.clone();
    let pb_clone = Arc::clone(&playback);
    let tap_clone = Arc::clone(&aec_tap);
    let needs_clone = Arc::clone(&needs_aec);
    let handle = std::thread::spawn(move || {
        run_inbound_audio_rx_loop(s_clone, &mut rx, pb_clone, tap_clone, needs_clone);
    });

    // Deliver out of order (100, 102, 101), drop 103 (FEC from 104), then 104..109.
    let send_order = [0usize, 2, 1, 4, 5, 6, 7, 8, 9];
    for idx in send_order {
        tx.send(InboundAudioPacket {
            seq: 100 + idx as u16,
            payload: packets[idx].clone(),
        })
        .unwrap();
    }

    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if playback.is_primed() && !aec_tap.is_empty() && playback.len() >= FRAME_20MS * 8 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    shared.running.store(false, Ordering::Release);
    handle.join().unwrap();

    {
        assert!(
            playback.is_primed(),
            "playback buffer should be primed after decoding Opus packets"
        );
        let mut rendered = vec![0.0f32; FRAME_20MS * 3];
        playback.pull_into(&mut rendered);
        assert!(
            compute_rms_mono(&rendered) > 0.01,
            "decoded playback audio should contain non-silent tone"
        );
    }
    {
        assert!(
            !aec_tap.is_empty(),
            "aec_render_tap should receive decoded samples"
        );
        assert!(
            aec_tap.len() <= aec_tap.capacity(),
            "aec_render_tap must be bounded by ring capacity, got {}",
            aec_tap.len()
        );
    }

    // Case 2: needs_sw_aec_ref = false -> aec_render_tap stays empty while playback still works.
    let shared2 = AudioSharedState::from_config(&cfg);
    let (tx2, mut rx2) = mpsc::unbounded_channel();
    let pb2 = Arc::new(PlaybackBuffer::new());
    let tap2 = Arc::new(SpscRing::<f32>::new(FRAME_20MS * 8));
    let needs2 = Arc::new(AtomicBool::new(false));
    let s2 = shared2.clone();
    let pb2_c = Arc::clone(&pb2);
    let tap2_c = Arc::clone(&tap2);
    let needs2_c = Arc::clone(&needs2);
    let h2 = std::thread::spawn(move || {
        run_inbound_audio_rx_loop(s2, &mut rx2, pb2_c, tap2_c, needs2_c);
    });
    for (i, pkt) in packets.iter().take(4).enumerate() {
        tx2.send(InboundAudioPacket {
            seq: i as u16,
            payload: pkt.clone(),
        })
        .unwrap();
    }
    let dl2 = Instant::now() + Duration::from_secs(2);
    while Instant::now() < dl2 {
        if pb2.is_primed() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    shared2.running.store(false, Ordering::Release);
    h2.join().unwrap();
    assert!(pb2.is_primed());
    assert!(tap2.is_empty());
}

#[test]
#[ignore = "benchmark; run with --release -- --ignored --nocapture"]
fn bench_audio_jitter_and_opus_pipeline() {
    const FRAMES: usize = 500;
    let mut enc = Encoder::new(SAMPLE_RATE, Channels::Mono, Application::Voip).unwrap();
    let _ = enc.set_bitrate(Bitrate::Bits(32_000));
    let _ = enc.set_vbr(true);
    let _ = enc.set_inband_fec(true);
    let _ = enc.set_packet_loss_perc(5);

    let mut dec = Decoder::new(SAMPLE_RATE, Channels::Mono).unwrap();
    let mut jitter = AudioJitterBuffer::new();
    let mut normalizer = LoudnessNormalizer::speech();

    // Pre-generate 20ms speech-like multi-tone PCM frames so benchmark measures DSP + codec + jitter.
    let pcm_frames: Vec<Vec<f32>> = (0..FRAMES)
        .map(|f| {
            (0..FRAME_20MS)
                .map(|i| {
                    let t = (f * FRAME_20MS + i) as f32 / SAMPLE_RATE as f32;
                    ((2.0 * std::f32::consts::PI * 260.0 * t).sin() * 0.18)
                        + ((2.0 * std::f32::consts::PI * 520.0 * t).sin() * 0.08)
                })
                .collect()
        })
        .collect();

    let mut enc_buf = vec![0u8; 4000];
    let mut dec_buf = vec![0.0f32; FRAME_20MS * 2];
    let mut decoded_frames = 0usize;
    let mut fec_count = 0usize;
    let mut plc_count = 0usize;
    let mut pending_swap: Option<(u16, Bytes)> = None;

    let t0 = Instant::now();
    let mut sim_now = t0;

    for (idx, pcm) in pcm_frames.iter().enumerate() {
        let n = enc.encode_float(pcm, &mut enc_buf).unwrap();
        let pkt = Bytes::copy_from_slice(&enc_buf[..n]);
        let seq = idx as u16;
        sim_now += Duration::from_millis(20);

        // Simulate 2% single-packet drop (exercises in-band Opus FEC) and 2% adjacent reorder.
        if idx > 2 && idx % 50 == 25 {
            // Drop packet `seq`; next packet will trigger FEC recovery after 35ms.
        } else if idx > 2 && idx % 50 == 10 && pending_swap.is_none() {
            pending_swap = Some((seq, pkt));
        } else {
            jitter.push(seq, pkt, sim_now);
            if let Some((delayed_seq, delayed_pkt)) = pending_swap.take() {
                jitter.push(delayed_seq, delayed_pkt, sim_now + Duration::from_millis(4));
            }
        }

        let drain_now = sim_now + Duration::from_millis(40);
        while let Some(step) = jitter.pop_ready(drain_now) {
            let decoded_res = match step {
                JitterStep::Packet(bytes) => dec.decode_float(&bytes, &mut dec_buf, false),
                JitterStep::FecFromNext(next) => {
                    fec_count += 1;
                    dec.decode_float(&next, &mut dec_buf[..FRAME_20MS], true)
                }
                JitterStep::Plc => {
                    plc_count += 1;
                    decode_opus_or_plc(&mut dec, &[], &mut dec_buf)
                }
            };
            if let Ok(samples) = decoded_res {
                normalizer.process_frame(&mut dec_buf[..samples]);
                decoded_frames += 1;
            }
        }
    }

    let elapsed = t0.elapsed();
    let total_ms = elapsed.as_secs_f64() * 1000.0;
    let fps = decoded_frames as f64 / elapsed.as_secs_f64().max(1e-9);
    let ms_per_frame = total_ms / decoded_frames.max(1) as f64;
    let realtime_x = (decoded_frames as f64 * 20.0) / total_ms.max(1e-6);
    println!(
        "bench_audio_jitter_and_opus_pipeline: {decoded_frames} frames (20ms @ 48kHz) in {total_ms:.2} ms | {fps:.1} frames/sec | {ms_per_frame:.3} ms/frame ({realtime_x:.1}x realtime, fec={fec_count}, plc={plc_count})"
    );
    assert!(
        decoded_frames >= FRAMES - 5,
        "expected near-complete decode with FEC, got {decoded_frames}/{FRAMES}"
    );
    assert!(fec_count > 0, "benchmark should exercise Opus FEC recovery");
}
