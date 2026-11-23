//! Pure-Rust audio pipeline: `cpal` capture/playback + `sonora` (AEC3 / NS / AGC2) + `opus` 48kHz.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _};
use cpal::{SampleFormat, Stream, StreamConfig};
use opus::{Application, Channels, Decoder, Encoder};
use sonora::config::{Config as SonoraConfig, EchoCanceller, GainController2, NoiseSuppression};
use sonora::{AudioProcessing, StreamConfig as SonoraStreamConfig};
use tokio::sync::mpsc;

use crate::{HuddleAudioConfig, HuddleError, Result};

/// Chime / Opus native sample rate (48 kHz).
pub const SAMPLE_RATE: u32 = 48_000;
/// 10ms frame size for `sonora` audio processing (480 samples at 48 kHz).
pub const FRAME_10MS: usize = 480;
/// 20ms frame size for Opus encoding/decoding (960 samples at 48 kHz).
pub const FRAME_20MS: usize = 960;

/// One enumerated audio device (mic or speaker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub is_default: bool,
}

/// Available input and output audio devices on the current host.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioDevices {
    pub inputs: Vec<AudioDeviceInfo>,
    pub outputs: Vec<AudioDeviceInfo>,
}

/// Enumerate all host microphones and speakers.
pub fn list_audio_devices() -> AudioDevices {
    let host = cpal::default_host();
    let default_in = host.default_input_device().and_then(|d| d.name().ok());
    let default_out = host.default_output_device().and_then(|d| d.name().ok());

    let inputs = host
        .input_devices()
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|d| d.name().ok())
        .map(|name| AudioDeviceInfo {
            is_default: default_in.as_deref() == Some(name.as_str()),
            name,
        })
        .collect();

    let outputs = host
        .output_devices()
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|d| d.name().ok())
        .map(|name| AudioDeviceInfo {
            is_default: default_out.as_deref() == Some(name.as_str()),
            name,
        })
        .collect();

    AudioDevices { inputs, outputs }
}

/// Distinct acoustic chimes for huddle lifecycle events (`F-COLLAB-060`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HuddleChime {
    /// Warm ascending triad when joining a huddle.
    Join,
    /// Gentle descending two-tone when leaving a huddle.
    Leave,
    /// Two-pulse ring chime for an incoming DM huddle invite.
    InviteRing,
}

/// Play a short synthesized huddle chime on `output_device` (or the default speaker) without
/// blocking the caller.
pub fn play_huddle_chime(chime: HuddleChime, output_device: Option<&str>) -> Result<()> {
    let device_name = output_device.map(ToOwned::to_owned);
    std::thread::Builder::new()
        .name("slackless-huddle-chime".to_owned())
        .spawn(move || {
            let Ok(device) = select_output_device(device_name.as_deref()) else {
                return;
            };
            let Ok(supported) = device.default_output_config() else {
                return;
            };
            let rate = supported.sample_rate().0 as f32;
            let channels = supported.channels() as usize;
            let duration_secs: f32 = match chime {
                HuddleChime::Join => 0.34,
                HuddleChime::Leave => 0.28,
                HuddleChime::InviteRing => 0.44,
            };
            let total_frames = (rate * duration_secs) as usize;
            let frame_ix = Arc::new(AtomicU32::new(0));
            let ix_cb = Arc::clone(&frame_ix);

            let config: StreamConfig = supported.into();
            let Ok(stream) = device.build_output_stream(
                &config,
                move |data: &mut [f32], _| {
                    let mut ix = ix_cb.load(Ordering::Relaxed) as usize;
                    for frame in data.chunks_mut(channels) {
                        let sample = if ix < total_frames {
                            let t = ix as f32 / rate;
                            let (freq, gain) = match chime {
                                HuddleChime::Join => {
                                    let f = if t < 0.11 {
                                        523.25 // C5
                                    } else if t < 0.22 {
                                        659.25 // E5
                                    } else {
                                        783.99 // G5
                                    };
                                    (f, 0.18)
                                }
                                HuddleChime::Leave => {
                                    let f = if t < 0.13 { 587.33 } else { 440.0 }; // D5 -> A4
                                    (f, 0.16)
                                }
                                HuddleChime::InviteRing => {
                                    let pulse = (t < 0.18) || (t > 0.24 && t < 0.42);
                                    let f = if t < 0.18 { 783.99 } else { 987.77 }; // G5 -> B5
                                    (f, if pulse { 0.20 } else { 0.0 })
                                }
                            };
                            let env = (1.0 - (t / duration_secs)).clamp(0.0, 1.0) * gain;
                            (2.0 * std::f32::consts::PI * freq * t).sin() * env
                        } else {
                            0.0
                        };
                        for ch in frame.iter_mut() {
                            *ch = sample;
                        }
                        ix += 1;
                    }
                    ix_cb.store(ix as u32, Ordering::Relaxed);
                },
                |_| {},
                None,
            ) else {
                return;
            };
            if stream.play().is_ok() {
                std::thread::sleep(Duration::from_millis((duration_secs * 1000.0) as u64 + 80));
            }
            drop(stream);
        })
        .map_err(|e| HuddleError::Audio(e.to_string()))?;
    Ok(())
}

/// Play a short two-note harmonic chime on `output_device` (or the default speaker) for `F-COLLAB-040`.
pub fn play_speaker_test_tone(output_device: Option<&str>) -> Result<()> {
    let device_name = output_device.map(ToOwned::to_owned);
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<()>>(1);

    std::thread::Builder::new()
        .name("slackless-speaker-test".to_owned())
        .spawn(move || {
            let start = (|| -> Result<Stream> {
                let device = select_output_device(device_name.as_deref())?;
                let supported = device
                    .default_output_config()
                    .map_err(|e| HuddleError::Audio(e.to_string()))?;
                let rate = supported.sample_rate().0 as f32;
                let channels = supported.channels() as usize;
                let total_frames = (rate * 0.45) as usize;
                let frame_ix = Arc::new(AtomicU32::new(0));
                let ix_cb = Arc::clone(&frame_ix);

                let config: StreamConfig = supported.into();
                let stream = device
                    .build_output_stream(
                        &config,
                        move |data: &mut [f32], _| {
                            let mut ix = ix_cb.load(Ordering::Relaxed) as usize;
                            for frame in data.chunks_mut(channels) {
                                let sample = if ix < total_frames {
                                    let t = ix as f32 / rate;
                                    let freq = if t < 0.22 { 523.25 } else { 659.25 }; // C5 -> E5
                                    let env = (1.0 - (t / 0.45)).clamp(0.0, 1.0) * 0.22;
                                    (2.0 * std::f32::consts::PI * freq * t).sin() * env
                                } else {
                                    0.0
                                };
                                for ch in frame.iter_mut() {
                                    *ch = sample;
                                }
                                ix += 1;
                            }
                            ix_cb.store(ix as u32, Ordering::Relaxed);
                        },
                        |_| {},
                        None,
                    )
                    .map_err(|e| HuddleError::Audio(e.to_string()))?;

                stream
                    .play()
                    .map_err(|e| HuddleError::Audio(e.to_string()))?;
                Ok(stream)
            })();

            match start {
                Ok(stream) => {
                    let _ = ready_tx.send(Ok(()));
                    std::thread::sleep(Duration::from_millis(520));
                    drop(stream);
                }
                Err(err) => {
                    let _ = ready_tx.send(Err(err));
                }
            }
        })
        .map_err(|e| HuddleError::Audio(e.to_string()))?;

    ready_rx
        .recv()
        .map_err(|e| HuddleError::Audio(e.to_string()))?
}

/// Lightweight live microphone level probe for Preferences -> Audio & video (`F-COLLAB-048`).
pub struct MicLevelProbe {
    running: Arc<AtomicBool>,
    rms_bits: Arc<AtomicU32>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl MicLevelProbe {
    pub fn start(input_device: Option<&str>) -> Result<Self> {
        let device_name = input_device.map(ToOwned::to_owned);
        let running = Arc::new(AtomicBool::new(true));
        let rms_bits = Arc::new(AtomicU32::new(0));
        let run_flag = Arc::clone(&running);
        let rms_cb = Arc::clone(&rms_bits);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<()>>(1);

        let worker = std::thread::Builder::new()
            .name("slackless-mic-probe".to_owned())
            .spawn(move || {
                let start = (|| -> Result<Stream> {
                    let device = select_input_device(device_name.as_deref())?;
                    let supported = device
                        .default_input_config()
                        .map_err(|e| HuddleError::Audio(e.to_string()))?;
                    let channels = supported.channels() as usize;
                    let config: StreamConfig = supported.into();

                    let stream = device
                        .build_input_stream(
                            &config,
                            move |data: &[f32], _| {
                                let rms = compute_rms_interleaved(data, channels);
                                rms_cb.store(rms.to_bits(), Ordering::Relaxed);
                            },
                            |_| {},
                            None,
                        )
                        .map_err(|e| HuddleError::Audio(e.to_string()))?;
                    stream
                        .play()
                        .map_err(|e| HuddleError::Audio(e.to_string()))?;
                    Ok(stream)
                })();

                match start {
                    Ok(stream) => {
                        let _ = ready_tx.send(Ok(()));
                        while run_flag.load(Ordering::Relaxed) {
                            std::thread::sleep(Duration::from_millis(25));
                        }
                        drop(stream);
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                    }
                }
            })
            .map_err(|e| HuddleError::Audio(e.to_string()))?;

        ready_rx
            .recv()
            .map_err(|e| HuddleError::Audio(e.to_string()))??;

        Ok(Self {
            running,
            rms_bits,
            worker: Some(worker),
        })
    }

    /// Current microphone RMS level normalized to `0.0..=1.0`.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.rms_bits.load(Ordering::Relaxed))
    }
}

impl Drop for MicLevelProbe {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

/// Shared runtime controls for a live [`AudioEngine`].
#[derive(Clone)]
pub struct AudioSharedState {
    pub muted: Arc<AtomicBool>,
    pub aec: Arc<AtomicBool>,
    pub ns: Arc<AtomicBool>,
    pub agc: Arc<AtomicBool>,
    pub local_rms_bits: Arc<AtomicU32>,
    pub running: Arc<AtomicBool>,
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
    worker: Option<std::thread::JoinHandle<mpsc::UnboundedReceiver<Vec<u8>>>>,
}

impl AudioEngine {
    pub fn start(
        cfg: &HuddleAudioConfig,
        shared: AudioSharedState,
        outbound_opus_tx: mpsc::UnboundedSender<Vec<u8>>,
        mut inbound_opus_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    ) -> Result<Self> {
        shared.running.store(true, Ordering::Relaxed);
        let capture_ring: Arc<Mutex<VecDeque<f32>>> =
            Arc::new(Mutex::new(VecDeque::with_capacity(FRAME_20MS * 8)));
        let playback_ring: Arc<Mutex<VecDeque<f32>>> =
            Arc::new(Mutex::new(VecDeque::with_capacity(FRAME_20MS * 10)));

        let in_name = cfg.input_device.clone();
        let out_name = cfg.output_device.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<()>>(1);

        let worker_shared = shared.clone();
        let worker = std::thread::Builder::new()
            .name("slackless-huddle-audio".to_owned())
            .spawn(move || {
                let streams = (|| -> Result<(Stream, Stream)> {
                    let in_stream =
                        open_input_stream(in_name.as_deref(), Arc::clone(&capture_ring))?;
                    let out_stream =
                        open_output_stream(out_name.as_deref(), Arc::clone(&playback_ring))?;
                    Ok((in_stream, out_stream))
                })();

                match streams {
                    Ok((in_stream, out_stream)) => {
                        let _ = ready_tx.send(Ok(()));
                        run_audio_dsp_loop(
                            worker_shared,
                            capture_ring,
                            playback_ring,
                            outbound_opus_tx,
                            &mut inbound_opus_rx,
                        );
                        drop(in_stream);
                        drop(out_stream);
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                    }
                }
                inbound_opus_rx
            })
            .map_err(|e| HuddleError::Audio(e.to_string()))?;

        if let Err(err) = ready_rx
            .recv()
            .map_err(|e| HuddleError::Audio(e.to_string()))
            .and_then(|r| r)
        {
            shared.running.store(false, Ordering::Relaxed);
            let _ = worker.join();
            return Err(err);
        }

        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }

    /// Stop streams & DSP thread, returning the inbound Opus receiver so a hot-swapped device
    /// restart keeps receiving remote audio seamlessly.
    pub fn stop(&mut self) -> Option<mpsc::UnboundedReceiver<Vec<u8>>> {
        self.shared.running.store(false, Ordering::Relaxed);
        self.worker.take().and_then(|handle| handle.join().ok())
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn build_sonora(aec: bool, ns: bool, agc: bool) -> AudioProcessing {
    let mut cfg = SonoraConfig::default();
    if aec {
        cfg.echo_canceller = Some(EchoCanceller::default());
    }
    if ns {
        cfg.noise_suppression = Some(NoiseSuppression::default());
    }
    if agc {
        cfg.gain_controller2 = Some(GainController2::default());
    }
    AudioProcessing::builder()
        .config(cfg)
        .capture_config(SonoraStreamConfig::new(SAMPLE_RATE, 1))
        .render_config(SonoraStreamConfig::new(SAMPLE_RATE, 1))
        .build()
}

fn run_audio_dsp_loop(
    shared: AudioSharedState,
    capture_ring: Arc<Mutex<VecDeque<f32>>>,
    playback_ring: Arc<Mutex<VecDeque<f32>>>,
    outbound_opus_tx: mpsc::UnboundedSender<Vec<u8>>,
    inbound_opus_rx: &mut mpsc::UnboundedReceiver<Vec<u8>>,
) {
    let Ok(mut encoder) = Encoder::new(SAMPLE_RATE, Channels::Mono, Application::Voip) else {
        return;
    };
    let Ok(mut decoder) = Decoder::new(SAMPLE_RATE, Channels::Mono) else {
        return;
    };

    let mut cur_aec = shared.aec.load(Ordering::Relaxed);
    let mut cur_ns = shared.ns.load(Ordering::Relaxed);
    let mut cur_agc = shared.agc.load(Ordering::Relaxed);
    let mut apm = build_sonora(cur_aec, cur_ns, cur_agc);

    let mut decoded_pcm = vec![0.0f32; FRAME_20MS * 2];
    let mut opus_out = vec![0u8; 4000];
    let silence_pcm = vec![0.0f32; FRAME_20MS];
    let mut processed_capture = Vec::with_capacity(FRAME_20MS);

    while shared.running.load(Ordering::Relaxed) {
        // Reconfigure Sonora live if AEC/NS/AGC toggles changed.
        let next_aec = shared.aec.load(Ordering::Relaxed);
        let next_ns = shared.ns.load(Ordering::Relaxed);
        let next_agc = shared.agc.load(Ordering::Relaxed);
        if (next_aec, next_ns, next_agc) != (cur_aec, cur_ns, cur_agc) {
            (cur_aec, cur_ns, cur_agc) = (next_aec, next_ns, next_agc);
            apm = build_sonora(cur_aec, cur_ns, cur_agc);
        }

        // 1. Drain inbound Opus packets -> decode -> feed AEC3 render reference -> playback ring.
        while let Ok(pkt) = inbound_opus_rx.try_recv() {
            if let Ok(samples) = decoder.decode_float(&pkt, &mut decoded_pcm, false) {
                let pcm = &decoded_pcm[..samples];
                for chunk in pcm.chunks(FRAME_10MS) {
                    if chunk.len() == FRAME_10MS {
                        let mut out_ref = [0.0f32; FRAME_10MS];
                        let _ = apm.process_render_f32(&[chunk], &mut [&mut out_ref[..]]);
                    }
                }
                if let Ok(mut ring) = playback_ring.lock() {
                    if ring.len() > FRAME_20MS * 8 {
                        let drop_n = ring.len() - FRAME_20MS * 4;
                        ring.drain(..drop_n);
                    }
                    ring.extend(pcm.iter().copied());
                }
            }
        }

        // 2. Drain captured 20ms mic chunks -> Sonora 10ms AEC3/NS/AGC -> Opus encode -> RTC TX.
        let mut mic_frame = None;
        if let Ok(mut ring) = capture_ring.lock() {
            if ring.len() >= FRAME_20MS {
                let mut frame = Vec::with_capacity(FRAME_20MS);
                for _ in 0..FRAME_20MS {
                    if let Some(s) = ring.pop_front() {
                        frame.push(s);
                    }
                }
                mic_frame = Some(frame);
            }
        }

        if let Some(frame) = mic_frame {
            let muted = shared.muted.load(Ordering::Relaxed);
            if muted {
                shared
                    .local_rms_bits
                    .store(0.0f32.to_bits(), Ordering::Relaxed);
                if let Ok(n) = encoder.encode_float(&silence_pcm, &mut opus_out) {
                    let _ = outbound_opus_tx.send(opus_out[..n].to_vec());
                }
            } else {
                processed_capture.clear();
                for chunk in frame.as_chunks::<FRAME_10MS>().0 {
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
                let rms = compute_rms_mono(&processed_capture);
                shared
                    .local_rms_bits
                    .store(rms.to_bits(), Ordering::Relaxed);
                if let Ok(n) = encoder.encode_float(&processed_capture, &mut opus_out) {
                    let _ = outbound_opus_tx.send(opus_out[..n].to_vec());
                }
            }
        } else {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

fn select_input_device(name: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    if let Some(target) = name.filter(|s| !s.is_empty()) {
        if let Ok(mut devices) = host.input_devices() {
            if let Some(found) = devices.find(|d| d.name().ok().as_deref() == Some(target)) {
                return Ok(found);
            }
        }
    }
    host.default_input_device()
        .ok_or_else(|| HuddleError::Audio("no input microphone device found".to_owned()))
}

fn select_output_device(name: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    if let Some(target) = name.filter(|s| !s.is_empty()) {
        if let Ok(mut devices) = host.output_devices() {
            if let Some(found) = devices.find(|d| d.name().ok().as_deref() == Some(target)) {
                return Ok(found);
            }
        }
    }
    host.default_output_device()
        .ok_or_else(|| HuddleError::Audio("no output speaker device found".to_owned()))
}

fn open_input_stream(name: Option<&str>, ring: Arc<Mutex<VecDeque<f32>>>) -> Result<Stream> {
    let device = select_input_device(name)?;
    let supported = device
        .default_input_config()
        .map_err(|e| HuddleError::Audio(e.to_string()))?;
    let src_rate = supported.sample_rate().0;
    let channels = supported.channels() as usize;
    let fmt = supported.sample_format();
    let config: StreamConfig = supported.into();

    let push_mono = move |mono: &[f32]| {
        let resampled = resample_linear(mono, src_rate, SAMPLE_RATE);
        if let Ok(mut q) = ring.lock() {
            if q.len() > FRAME_20MS * 8 {
                let drop_n = q.len() - FRAME_20MS * 4;
                q.drain(..drop_n);
            }
            q.extend(resampled);
        }
    };

    let stream = match fmt {
        SampleFormat::F32 => device.build_input_stream(
            &config,
            move |data: &[f32], _| {
                let mono = downmix_to_mono_f32(data, channels);
                push_mono(&mono);
            },
            |_| {},
            None,
        ),
        SampleFormat::I16 => device.build_input_stream(
            &config,
            move |data: &[i16], _| {
                let mono: Vec<f32> = data
                    .chunks(channels)
                    .map(|ch| {
                        ch.iter().map(|&s| s as f32 / 32768.0).sum::<f32>() / channels.max(1) as f32
                    })
                    .collect();
                push_mono(&mono);
            },
            |_| {},
            None,
        ),
        other => {
            return Err(HuddleError::Audio(format!(
                "unsupported input sample format {other:?}"
            )));
        }
    }
    .map_err(|e| HuddleError::Audio(e.to_string()))?;

    stream
        .play()
        .map_err(|e| HuddleError::Audio(e.to_string()))?;
    Ok(stream)
}

fn open_output_stream(name: Option<&str>, ring: Arc<Mutex<VecDeque<f32>>>) -> Result<Stream> {
    let device = select_output_device(name)?;
    let supported = device
        .default_output_config()
        .map_err(|e| HuddleError::Audio(e.to_string()))?;
    let dst_rate = supported.sample_rate().0;
    let channels = supported.channels() as usize;
    let config: StreamConfig = supported.into();

    let stream = device
        .build_output_stream(
            &config,
            move |data: &mut [f32], _| {
                let out_frames = data.len() / channels.max(1);
                let needed_48k = ((out_frames as u64 * SAMPLE_RATE as u64)
                    .div_ceil(dst_rate.max(1) as u64)) as usize;
                let mut src = Vec::with_capacity(needed_48k);
                if let Ok(mut q) = ring.lock() {
                    for _ in 0..needed_48k {
                        src.push(q.pop_front().unwrap_or(0.0));
                    }
                } else {
                    src.resize(needed_48k, 0.0);
                }
                let resampled = resample_linear(&src, SAMPLE_RATE, dst_rate);
                for (i, frame) in data.chunks_mut(channels).enumerate() {
                    let s = resampled.get(i).copied().unwrap_or(0.0).clamp(-1.0, 1.0);
                    for ch in frame.iter_mut() {
                        *ch = s;
                    }
                }
            },
            |_| {},
            None,
        )
        .map_err(|e| HuddleError::Audio(e.to_string()))?;

    stream
        .play()
        .map_err(|e| HuddleError::Audio(e.to_string()))?;
    Ok(stream)
}

fn downmix_to_mono_f32(data: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data.to_vec();
    }
    data.chunks(channels)
        .map(|ch| ch.iter().copied().sum::<f32>() / channels as f32)
        .collect()
}

/// Linear resampler used when a macOS device runs at 44.1 kHz instead of 48 kHz.
pub fn resample_linear(input: &[f32], src_rate: u32, dst_rate: u32) -> Vec<f32> {
    if src_rate == dst_rate || input.is_empty() {
        return input.to_vec();
    }
    let ratio = src_rate as f64 / dst_rate as f64;
    let out_len = ((input.len() as f64) / ratio).round().max(1.0) as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as f64 * ratio;
        let idx = pos.floor() as usize;
        let frac = (pos - idx as f64) as f32;
        let s0 = input.get(idx).copied().unwrap_or(0.0);
        let s1 = input.get(idx + 1).copied().unwrap_or(s0);
        out.push(s0 + (s1 - s0) * frac);
    }
    out
}

pub fn compute_rms_mono(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt().clamp(0.0, 1.0)
}

fn compute_rms_interleaved(samples: &[f32], channels: usize) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let mono = downmix_to_mono_f32(samples, channels);
    compute_rms_mono(&mono)
}

/// Pure-Rust gentle ambient lo-fi chord player for huddle jukebox / "music when alone"
/// (`F-COLLAB-036`, `F-COLLAB-058`). Runs on its own OS thread so `cpal::Stream` stays off UI.
pub struct AmbientMusicPlayer {
    running: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl AmbientMusicPlayer {
    pub fn start(output_device: Option<String>) -> Result<Self> {
        let running = Arc::new(AtomicBool::new(true));
        let run_flag = Arc::clone(&running);
        let worker = std::thread::Builder::new()
            .name("slackless-huddle-music".to_owned())
            .spawn(move || {
                let Ok(device) = select_output_device(output_device.as_deref()) else {
                    return;
                };
                let Ok(supported) = device.default_output_config() else {
                    return;
                };
                let rate = supported.sample_rate().0 as f32;
                let channels = supported.channels() as usize;
                let config: StreamConfig = supported.into();
                // Warm lo-fi 4-chord progression (Cmaj7 -> Am7 -> Fmaj7 -> G6)
                const CHORDS: [[f32; 4]; 4] = [
                    [261.63, 329.63, 392.00, 493.88],
                    [220.00, 261.63, 329.63, 392.00],
                    [174.61, 220.00, 261.63, 329.63],
                    [196.00, 246.94, 293.66, 329.63],
                ];
                let mut sample_idx: u64 = 0;
                let chord_samples = (rate * 3.5) as u64;
                let stream = device.build_output_stream(
                    &config,
                    move |out: &mut [f32], _| {
                        for frame in out.chunks_mut(channels) {
                            let chord_ix = ((sample_idx / chord_samples) % 4) as usize;
                            let pos_in_chord =
                                (sample_idx % chord_samples) as f32 / chord_samples as f32;
                            let env = (pos_in_chord * 8.0).min(1.0)
                                * ((1.0 - pos_in_chord) * 8.0).min(1.0);
                            let t = sample_idx as f32 / rate;
                            let mut s = 0.0f32;
                            for &hz in &CHORDS[chord_ix] {
                                s += (2.0 * std::f32::consts::PI * hz * t).sin();
                            }
                            let sample = (s * 0.018 * env).clamp(-0.12, 0.12);
                            for slot in frame {
                                *slot = sample;
                            }
                            sample_idx = sample_idx.wrapping_add(1);
                        }
                    },
                    |_| {},
                    None,
                );
                let Ok(stream) = stream else {
                    return;
                };
                if stream.play().is_err() {
                    return;
                }
                while run_flag.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(80));
                }
            })
            .map_err(|e| HuddleError::Audio(e.to_string()))?;
        Ok(Self {
            running,
            worker: Some(worker),
        })
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for AmbientMusicPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn linear_resampler_scales_length_accurately() {
        let input = vec![0.5f32; 441];
        let out = resample_linear(&input, 44_100, 48_000);
        assert_eq!(out.len(), 480);
        assert!((out[240] - 0.5).abs() < 1e-4);
    }
}
