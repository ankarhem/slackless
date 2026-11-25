use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};
use opus::{Application, Bitrate, Channels, Decoder, Encoder};
use tokio::sync::mpsc;

use super::devices::{current_default_input_name, current_default_output_name};
use super::dsp::{LoudnessNormalizer, build_sonora, compute_rms_mono};
use super::ring::SpscRing;
use super::streams::{
    AudioIoBackend, CaptureRing, open_hw_duplex, open_input_stream, open_output_stream,
};
use super::{
    AudioJitterBuffer, FRAME_10MS, FRAME_20MS, JitterStep, PlaybackBuffer, SAMPLE_RATE,
    decode_opus_or_plc,
};
use crate::backend::{AudioDeviceChangeKind, platform_backend};
use crate::rtc::InboundAudioPacket;
use crate::{HuddleAudioConfig, HuddleError, Result};

/// Shared runtime controls for a live [`AudioEngine`].
#[derive(Clone)]
pub struct AudioSharedState {
    pub muted: Arc<AtomicBool>,
    pub aec: Arc<AtomicBool>,
    pub ns: Arc<AtomicBool>,
    pub agc: Arc<AtomicBool>,
    pub local_rms_bits: Arc<AtomicU32>,
    pub running: Arc<AtomicBool>,
    /// Optional opt-in local multilingual Whisper transcriber tap (zero overhead when `None`).
    pub transcribe_tap: Arc<Mutex<Option<Arc<crate::transcribe::LocalTranscriber>>>>,
    /// Local participant user ID for tagging local microphone utterances.
    pub self_user: Arc<Mutex<slackless_types::UserId>>,
    /// Loudest active remote speaker for attributing remote audio utterances.
    pub active_remote_speaker: Arc<Mutex<Option<slackless_types::UserId>>>,
}

impl AudioSharedState {
    pub fn from_config(cfg: &HuddleAudioConfig) -> Self {
        Self {
            muted: Arc::new(AtomicBool::new(cfg.muted)),
            aec: Arc::new(AtomicBool::new(cfg.aec)),
            ns: Arc::new(AtomicBool::new(cfg.ns)),
            agc: Arc::new(AtomicBool::new(cfg.agc)),
            local_rms_bits: Arc::new(AtomicU32::new(0)),
            running: Arc::new(AtomicBool::new(true)),
            transcribe_tap: Arc::new(Mutex::new(None)),
            self_user: Arc::new(Mutex::new(slackless_types::UserId::from("U0SELF"))),
            active_remote_speaker: Arc::new(Mutex::new(None)),
        }
    }

    pub fn local_rms(&self) -> f32 {
        if self.muted.load(Ordering::Relaxed) {
            0.0
        } else {
            f32::from_bits(self.local_rms_bits.load(Ordering::Relaxed))
        }
    }
}

/// Live full-duplex audio engine (capture → sonora AEC3/NS/AGC → Opus TX; Opus RX → sonora ref → speaker).
///
/// Both `cpal::Stream`s live on the dedicated audio worker OS thread so `AudioEngine` stays `Send`
/// on macOS CoreAudio (where `cpal::Stream` is `!Send`).
pub struct AudioEngine {
    shared: AudioSharedState,
    capture_ring: Arc<CaptureRing>,
    worker: Option<std::thread::JoinHandle<()>>,
    rx_worker: Option<std::thread::JoinHandle<mpsc::UnboundedReceiver<InboundAudioPacket>>>,
}

impl AudioEngine {
    pub fn start(
        cfg: &HuddleAudioConfig,
        shared: AudioSharedState,
        outbound_opus_tx: mpsc::UnboundedSender<Bytes>,
        inbound_opus_rx: mpsc::UnboundedReceiver<InboundAudioPacket>,
    ) -> Result<Self> {
        Self::start_recoverable(cfg, shared, outbound_opus_tx, inbound_opus_rx)
            .map_err(|(err, _)| err)
    }

    /// Start the audio engine, returning `inbound_opus_rx` intact alongside the error if opening
    /// the audio device fails so live device hot-swaps never permanently kill session audio.
    pub(crate) fn start_recoverable(
        cfg: &HuddleAudioConfig,
        shared: AudioSharedState,
        outbound_opus_tx: mpsc::UnboundedSender<Bytes>,
        inbound_opus_rx: mpsc::UnboundedReceiver<InboundAudioPacket>,
    ) -> std::result::Result<Self, (HuddleError, mpsc::UnboundedReceiver<InboundAudioPacket>)> {
        shared.running.store(true, Ordering::Release);
        let capture_ring = Arc::new(CaptureRing::new());
        let playback_buf = Arc::new(PlaybackBuffer::new());
        // Optional lock-free render tap used only when software `cpal + sonora` fallback is active
        // (macOS CoreAudio `VoiceProcessingIO` taps its own render bus in hardware).
        let aec_render_tap = Arc::new(SpscRing::<f32>::new(FRAME_20MS * 8));
        let needs_sw_aec_ref = Arc::new(AtomicBool::new(false));

        let in_name = cfg.input_device.clone();
        let out_name = cfg.output_device.clone();
        let init_muted = shared.muted.load(Ordering::Relaxed);
        let init_aec = cfg.aec;
        let init_ns = cfg.ns;
        let init_agc = cfg.agc;
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<()>>(1);

        let worker_shared = shared.clone();
        let worker_capture = Arc::clone(&capture_ring);
        let worker_playback = Arc::clone(&playback_buf);
        let worker_aec_tap = Arc::clone(&aec_render_tap);
        let worker_needs_aec = Arc::clone(&needs_sw_aec_ref);
        let worker = match std::thread::Builder::new()
            .name("slackless-huddle-audio".to_owned())
            .spawn(move || {
                worker_capture.register_worker();
                let io_backend: Result<AudioIoBackend> = if let Some(hw) = open_hw_duplex(
                    in_name.as_deref(),
                    out_name.as_deref(),
                    !init_muted && init_aec,
                    !init_muted && init_ns,
                    !init_muted && init_agc,
                    &worker_capture,
                    &worker_playback,
                ) {
                    worker_needs_aec.store(false, Ordering::Release);
                    Ok(AudioIoBackend::Hardware(hw))
                } else {
                    (|| -> Result<AudioIoBackend> {
                        let in_stream =
                            open_input_stream(in_name.as_deref(), Arc::clone(&worker_capture))?;
                        let out_stream =
                            open_output_stream(out_name.as_deref(), Arc::clone(&worker_playback))?;
                        worker_needs_aec.store(true, Ordering::Release);
                        Ok(AudioIoBackend::Cpal {
                            in_stream,
                            out_stream,
                            apm: Box::new(build_sonora(init_aec, init_ns, init_agc)),
                        })
                    })()
                };

                match io_backend {
                    Ok(io) => {
                        let _ = ready_tx.send(Ok(()));
                        run_audio_dsp_loop(AudioDspLoopArgs {
                            shared: worker_shared,
                            capture_ring: worker_capture,
                            playback_buf: worker_playback,
                            aec_render_tap: worker_aec_tap,
                            outbound_opus_tx,
                            configured_in: in_name,
                            configured_out: out_name,
                            io,
                        });
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                    }
                }
            }) {
            Ok(h) => h,
            Err(e) => return Err((HuddleError::Audio(e.to_string()), inbound_opus_rx)),
        };

        if let Err(err) = ready_rx
            .recv()
            .map_err(|e| HuddleError::Audio(e.to_string()))
            .and_then(|r| r)
        {
            shared.running.store(false, Ordering::Release);
            capture_ring.wake_now();
            let _ = worker.join();
            return Err((err, inbound_opus_rx));
        }

        // Dedicated inbound Opus jitter-buffer + decoder thread: completely decoupled from the
        // microphone capture loop so remote speech is decoded within microseconds of UDP arrival!
        // Hand `inbound_opus_rx` across a 1-slot channel only after the thread spawns so a spawn
        // failure still returns `inbound_opus_rx` intact.
        let (rx_handoff_tx, rx_handoff_rx) =
            std::sync::mpsc::sync_channel::<mpsc::UnboundedReceiver<InboundAudioPacket>>(1);
        let rx_shared = shared.clone();
        let rx_playback = Arc::clone(&playback_buf);
        let rx_aec_tap = Arc::clone(&aec_render_tap);
        let rx_needs_aec = Arc::clone(&needs_sw_aec_ref);
        let rx_worker = match std::thread::Builder::new()
            .name("slackless-huddle-audio-rx".to_owned())
            .spawn(move || {
                let mut rx = match rx_handoff_rx.recv() {
                    Ok(rx) => rx,
                    Err(_) => {
                        let (_tx, dummy_rx) = mpsc::unbounded_channel();
                        return dummy_rx;
                    }
                };
                run_inbound_audio_rx_loop(
                    rx_shared,
                    &mut rx,
                    rx_playback,
                    rx_aec_tap,
                    rx_needs_aec,
                );
                rx
            }) {
            Ok(h) => h,
            Err(e) => {
                shared.running.store(false, Ordering::Release);
                capture_ring.wake_now();
                let _ = worker.join();
                return Err((HuddleError::Audio(e.to_string()), inbound_opus_rx));
            }
        };
        if let Err(std::sync::mpsc::SendError(rx_back)) = rx_handoff_tx.send(inbound_opus_rx) {
            shared.running.store(false, Ordering::Release);
            capture_ring.wake_now();
            let _ = worker.join();
            let _ = rx_worker.join();
            return Err((
                HuddleError::Audio("audio rx worker exited before handoff".to_owned()),
                rx_back,
            ));
        }

        Ok(Self {
            shared,
            capture_ring,
            worker: Some(worker),
            rx_worker: Some(rx_worker),
        })
    }

    /// Stop streams & DSP threads, returning the inbound Opus receiver so a hot-swapped device
    /// restart keeps receiving remote audio seamlessly.
    pub fn stop(&mut self) -> Option<mpsc::UnboundedReceiver<InboundAudioPacket>> {
        self.shared.running.store(false, Ordering::Release);
        self.capture_ring.wake_now();
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
        self.rx_worker.take().and_then(|handle| handle.join().ok())
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

pub(super) fn run_inbound_audio_rx_loop(
    shared: AudioSharedState,
    inbound_opus_rx: &mut mpsc::UnboundedReceiver<InboundAudioPacket>,
    playback_buf: Arc<PlaybackBuffer>,
    aec_render_tap: Arc<SpscRing<f32>>,
    needs_sw_aec_ref: Arc<AtomicBool>,
) {
    let Ok(mut decoder) = Decoder::new(SAMPLE_RATE, Channels::Mono) else {
        return;
    };
    let mut jitter = AudioJitterBuffer::new();
    let mut normalizer = LoudnessNormalizer::speech();
    let mut decoded_pcm = vec![0.0f32; FRAME_20MS * 2];

    while shared.running.load(Ordering::Acquire) {
        let mut received_any = false;
        while let Ok(pkt) = inbound_opus_rx.try_recv() {
            jitter.push(pkt.seq, pkt.payload, Instant::now());
            received_any = true;
        }

        let now = Instant::now();
        let mut decoded_any = false;
        while let Some(step) = jitter.pop_ready(now) {
            let decoded_res = match step {
                JitterStep::Packet(pkt) => decoder.decode_float(&pkt, &mut decoded_pcm, false),
                JitterStep::FecFromNext(next_pkt) => {
                    decoder.decode_float(&next_pkt, &mut decoded_pcm[..FRAME_20MS], true)
                }
                JitterStep::Plc => decode_opus_or_plc(&mut decoder, &[], &mut decoded_pcm),
            };
            if let Ok(samples) = decoded_res {
                let pcm = &mut decoded_pcm[..samples];
                // Generalized per-stream speech loudness normalization: lifts quiet mics and tames
                // hot mics to a consistent conversational level without boosting silence.
                normalizer.process_frame(pcm);
                playback_buf.push(pcm);
                if let Ok(guard) = shared.transcribe_tap.lock()
                    && let Some(tap) = guard.as_ref()
                {
                    let speaker = shared
                        .active_remote_speaker
                        .lock()
                        .ok()
                        .and_then(|s| s.clone())
                        .unwrap_or_else(|| slackless_types::UserId::from("U0REMOTE"));
                    tap.push(false, speaker, pcm);
                }
                if needs_sw_aec_ref.load(Ordering::Relaxed)
                    && !shared.muted.load(Ordering::Relaxed)
                    && shared.aec.load(Ordering::Relaxed)
                {
                    aec_render_tap.push_slice(pcm);
                }
                decoded_any = true;
            }
        }

        if !received_any && !decoded_any {
            // Short 5ms poll sleep when idle: wakes immediately enough for 20ms RTP cadence
            // while still checking `shared.running` cleanly on shutdown/device-swap.
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

struct AudioDspLoopArgs {
    shared: AudioSharedState,
    capture_ring: Arc<CaptureRing>,
    playback_buf: Arc<PlaybackBuffer>,
    aec_render_tap: Arc<SpscRing<f32>>,
    outbound_opus_tx: mpsc::UnboundedSender<Bytes>,
    configured_in: Option<String>,
    configured_out: Option<String>,
    io: AudioIoBackend,
}

fn run_audio_dsp_loop(args: AudioDspLoopArgs) {
    let AudioDspLoopArgs {
        shared,
        capture_ring,
        playback_buf,
        aec_render_tap,
        outbound_opus_tx,
        configured_in,
        configured_out,
        mut io,
    } = args;

    let Ok(mut encoder) = Encoder::new(SAMPLE_RATE, Channels::Mono, Application::Voip) else {
        return;
    };
    // Match Chime SDP `useinbandfec=1` with crisp 32 kbps VBR voice + DTX for low idle uplink.
    let _ = encoder.set_bitrate(Bitrate::Bits(32_000));
    let _ = encoder.set_vbr(true);
    let _ = encoder.set_inband_fec(true);
    let _ = encoder.set_packet_loss_perc(5);
    let _ = encoder.set_dtx(true);

    let mut cur_aec = shared.aec.load(Ordering::Relaxed);
    let mut cur_ns = shared.ns.load(Ordering::Relaxed);
    let mut cur_agc = shared.agc.load(Ordering::Relaxed);
    let mut cur_muted = shared.muted.load(Ordering::Relaxed);

    let mut opus_out = vec![0u8; 4000];
    // Chunked arena for outbound Opus packets: amortizes heap allocation to ~1 alloc/sec instead
    // of 50 Vec<u8> allocs/sec, and `cached_silence_opus.clone()` is a zero-alloc refcount bump.
    let mut opus_arena = BytesMut::with_capacity(8192);
    let silence_pcm = vec![0.0f32; FRAME_20MS];
    let cached_silence_opus: Bytes = encoder
        .encode_float(&silence_pcm, &mut opus_out)
        .map(|n| Bytes::copy_from_slice(&opus_out[..n]))
        .unwrap_or_else(|_| Bytes::from_static(&[0xF8, 0xFF, 0xFE]));
    let mut mic_frame = [0.0f32; FRAME_20MS];
    let mut processed_capture = Vec::with_capacity(FRAME_20MS);
    let mut mic_normalizer = LoudnessNormalizer::speech();
    let mut aec_ref_chunk = [0.0f32; FRAME_10MS];

    // Event-driven OS audio device hot-plug monitor (CoreAudio listener on macOS; falls back to
    // 1.5s polling only on platforms without a native listener).
    let in_dirty = Arc::new(AtomicBool::new(false));
    let out_dirty = Arc::new(AtomicBool::new(false));
    let device_monitor = {
        let in_d = Arc::clone(&in_dirty);
        let out_d = Arc::clone(&out_dirty);
        let wake = Arc::clone(&capture_ring);
        platform_backend().start_audio_device_monitor(Arc::new(move |kind| {
            match kind {
                AudioDeviceChangeKind::Input => in_d.store(true, Ordering::Release),
                AudioDeviceChangeKind::Output => out_d.store(true, Ordering::Release),
            }
            wake.wake_now();
        }))
    };
    let use_polling_hotplug = device_monitor.is_none();

    let mut last_default_in = if configured_in.is_none() {
        current_default_input_name()
    } else {
        None
    };
    let mut last_default_out = if configured_out.is_none() {
        current_default_output_name()
    } else {
        None
    };
    let mut last_device_check = Instant::now();

    while shared.running.load(Ordering::Acquire) {
        let check_in = in_dirty.swap(false, Ordering::AcqRel);
        let check_out = out_dirty.swap(false, Ordering::AcqRel);
        let poll_due =
            use_polling_hotplug && last_device_check.elapsed() >= Duration::from_millis(1500);
        if check_in || check_out || poll_due {
            if poll_due {
                last_device_check = Instant::now();
            }
            match &mut io {
                AudioIoBackend::Hardware(_) => {
                    // Restart hardware VoiceProcessingIO cleanly when the system default input or
                    // output changes (e.g. AirPods connect/disconnect).
                    let now_in = current_default_input_name();
                    let now_out = current_default_output_name();
                    if now_in != last_default_in || now_out != last_default_out {
                        last_default_in = now_in;
                        last_default_out = now_out;
                        if let Some(next_hw) = open_hw_duplex(
                            None,
                            None,
                            !cur_muted && cur_aec,
                            !cur_muted && cur_ns,
                            !cur_muted && cur_agc,
                            &capture_ring,
                            &playback_buf,
                        ) {
                            io = AudioIoBackend::Hardware(next_hw);
                        }
                    }
                }
                AudioIoBackend::Cpal {
                    in_stream,
                    out_stream,
                    ..
                } => {
                    if configured_in.is_none() && (check_in || poll_due) {
                        let now_in = current_default_input_name();
                        if now_in.is_some()
                            && now_in != last_default_in
                            && let Ok(next_stream) =
                                open_input_stream(None, Arc::clone(&capture_ring))
                        {
                            *in_stream = next_stream;
                            last_default_in = now_in;
                        }
                    }
                    if configured_out.is_none() && (check_out || poll_due) {
                        let now_out = current_default_output_name();
                        if now_out.is_some()
                            && now_out != last_default_out
                            && let Ok(next_stream) =
                                open_output_stream(None, Arc::clone(&playback_buf))
                        {
                            *out_stream = next_stream;
                            last_default_out = now_out;
                        }
                    }
                }
            }
        }

        // Reconfigure hardware DSP or Sonora live if AEC/NS/AGC or mute toggles changed.
        // Bypassing VoiceProcessingIO's AEC/NS/AGC while muted cuts idle muted audio CPU
        // significantly while keeping remote speaker playback untouched.
        let next_aec = shared.aec.load(Ordering::Relaxed);
        let next_ns = shared.ns.load(Ordering::Relaxed);
        let next_agc = shared.agc.load(Ordering::Relaxed);
        let next_muted = shared.muted.load(Ordering::Relaxed);
        let dsp_changed = (next_aec, next_ns, next_agc) != (cur_aec, cur_ns, cur_agc);
        let mute_changed = next_muted != cur_muted;
        if dsp_changed || mute_changed {
            (cur_aec, cur_ns, cur_agc, cur_muted) = (next_aec, next_ns, next_agc, next_muted);
            match &mut io {
                AudioIoBackend::Hardware(hw) => {
                    hw.set_processing(!cur_muted && (cur_aec || cur_ns), !cur_muted && cur_agc);
                }
                AudioIoBackend::Cpal { apm, .. } if dsp_changed => {
                    **apm = build_sonora(cur_aec, cur_ns, cur_agc);
                }
                AudioIoBackend::Cpal { .. } => {}
            }
        }

        // 1. Feed Sonora's AEC render reference only when running the software `cpal` fallback.
        // (Hardware `VoiceProcessingIO` taps its own render bus internally, and inbound Opus
        // decoding runs on the dedicated `slackless-huddle-audio-rx` thread without mic-loop lag.)
        if let AudioIoBackend::Cpal { apm, .. } = &mut io {
            if !cur_muted && cur_aec {
                let tap_len = aec_render_tap.len();
                if tap_len > FRAME_20MS * 6 {
                    aec_render_tap.skip(tap_len - FRAME_20MS * 3);
                }
                while aec_render_tap.len() >= FRAME_10MS {
                    if aec_render_tap.pop_slice(&mut aec_ref_chunk) < FRAME_10MS {
                        break;
                    }
                    let mut out_ref = [0.0f32; FRAME_10MS];
                    let _ = apm.process_render_f32(&[&aec_ref_chunk[..]], &mut [&mut out_ref[..]]);
                }
            } else {
                aec_render_tap.clear_consumer();
            }
        }

        // 2. Drain captured 20ms mic chunks -> (HW already processed or Sonora) -> Opus -> RTC TX.
        if capture_ring.wait_and_pop_frame(
            &mut mic_frame,
            &shared.running,
            Duration::from_millis(18),
        ) {
            let muted = shared.muted.load(Ordering::Relaxed);
            if muted {
                shared
                    .local_rms_bits
                    .store(0.0f32.to_bits(), Ordering::Relaxed);
                let _ = outbound_opus_tx.send(cached_silence_opus.clone());
            } else {
                let pcm_to_encode: &mut [f32] = match &mut io {
                    AudioIoBackend::Hardware(_) => &mut mic_frame,
                    AudioIoBackend::Cpal { apm, .. } => {
                        processed_capture.clear();
                        for chunk in mic_frame.as_chunks::<FRAME_10MS>().0 {
                            let mut out_chunk = [0.0f32; FRAME_10MS];
                            if apm
                                .process_capture_f32(&[chunk.as_slice()], &mut [&mut out_chunk[..]])
                                .is_ok()
                            {
                                processed_capture.extend_from_slice(&out_chunk);
                            } else {
                                processed_capture.extend_from_slice(chunk);
                            }
                        }
                        &mut processed_capture
                    }
                };
                // Normalize local microphone volume when AGC is enabled so quiet Bluetooth/USB mics
                // and hot desk mics both transmit at consistent conversational loudness.
                if cur_agc {
                    mic_normalizer.process_frame(pcm_to_encode);
                }
                let rms = compute_rms_mono(pcm_to_encode);
                if let Ok(guard) = shared.transcribe_tap.lock()
                    && let Some(tap) = guard.as_ref()
                {
                    let speaker = shared
                        .self_user
                        .lock()
                        .ok()
                        .map(|s| s.clone())
                        .unwrap_or_else(|| slackless_types::UserId::from("U0SELF"));
                    tap.push(true, speaker, pcm_to_encode);
                }
                shared
                    .local_rms_bits
                    .store(rms.to_bits(), Ordering::Relaxed);
                if let Ok(n) = encoder.encode_float(pcm_to_encode, &mut opus_out) {
                    if opus_arena.capacity() < n {
                        opus_arena = BytesMut::with_capacity(8192.max(n));
                    }
                    opus_arena.extend_from_slice(&opus_out[..n]);
                    let _ = outbound_opus_tx.send(opus_arena.split().freeze());
                }
            }
        }
    }
    drop(device_monitor);
    drop(io);
}
