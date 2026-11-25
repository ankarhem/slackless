use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use cpal::traits::{DeviceTrait as _, StreamTrait as _};
use cpal::{SampleFormat, Stream, StreamConfig, SupportedStreamConfig};
use sonora::AudioProcessing;

use super::devices::{select_input_device, select_output_device};
use super::dsp::{downmix_to_mono_f32_into, resample_linear_into};
use super::ring::SpscRing;
use super::{FRAME_20MS, PlaybackBuffer, SAMPLE_RATE};
use crate::backend::{HwAudioDuplexConfig, HwAudioDuplexHandle, platform_backend};
use crate::{HuddleError, Result};

/// Preallocated scratch capacity for `cpal` real-time callbacks so steady-state callbacks never
/// allocate on the heap.
const RT_SCRATCH_SAMPLES: usize = 16_384;

/// Lock-free capture ring shared between real-time audio input callbacks (`push`) and the
/// dedicated audio DSP worker thread (`wait_and_pop_frame`).
pub(super) struct CaptureRing {
    ring: SpscRing<f32>,
    worker: OnceLock<std::thread::Thread>,
    parked: AtomicBool,
}

impl CaptureRing {
    pub(super) fn new() -> Self {
        Self {
            ring: SpscRing::new(FRAME_20MS * 16),
            worker: OnceLock::new(),
            parked: AtomicBool::new(false),
        }
    }

    /// Called once from the DSP worker thread so RT producers can unpark it lock-free.
    pub(super) fn register_worker(&self) {
        let _ = self.worker.set(std::thread::current());
    }

    /// Producer (real-time audio callback): push 48 kHz mono PCM lock-free and wake the DSP
    /// worker only if it is currently parked.
    pub(super) fn push(&self, samples: &[f32]) {
        self.ring.push_slice(samples);
        if self.ring.len() >= FRAME_20MS {
            self.wake_if_parked();
        }
    }

    /// Lock-free wake for RT callbacks and CoreAudio device-change listeners.
    pub(super) fn wake_if_parked(&self) {
        if self.parked.swap(false, Ordering::AcqRel)
            && let Some(t) = self.worker.get()
        {
            t.unpark();
        }
    }

    /// Unconditional wake used on shutdown/hot-plug so the worker wakes immediately.
    pub(super) fn wake_now(&self) {
        self.parked.store(false, Ordering::Release);
        if let Some(t) = self.worker.get() {
            t.unpark();
        }
    }

    /// Consumer (DSP worker thread): wait up to `timeout` for a full 20 ms frame, trimming excess
    /// backlog on the consumer side to preserve the SPSC invariant and keep latency bounded.
    pub(super) fn wait_and_pop_frame(
        &self,
        dst: &mut [f32; FRAME_20MS],
        running: &AtomicBool,
        timeout: Duration,
    ) -> bool {
        if self.ring.len() < FRAME_20MS && running.load(Ordering::Acquire) {
            self.parked.store(true, Ordering::Release);
            if self.ring.len() < FRAME_20MS && running.load(Ordering::Acquire) {
                std::thread::park_timeout(timeout);
            }
            self.parked.store(false, Ordering::Release);
        }
        let len = self.ring.len();
        if len > FRAME_20MS * 8 {
            self.ring.skip(len - FRAME_20MS * 4);
        }
        if self.ring.len() >= FRAME_20MS {
            self.ring.pop_slice(dst) == FRAME_20MS
        } else {
            false
        }
    }
}

pub(super) enum AudioIoBackend {
    /// OS-native full-duplex 48 kHz hardware-tuned AEC/NS/AGC (e.g. macOS CoreAudio
    /// `VoiceProcessingIO`) — skips software `sonora` completely!
    Hardware(Box<dyn HwAudioDuplexHandle>),
    /// Cross-platform portable fallback (`cpal` + pure-Rust `sonora` AEC3/NS/AGC2).
    Cpal {
        in_stream: Stream,
        out_stream: Stream,
        apm: Box<AudioProcessing>,
    },
}

pub(super) fn open_hw_duplex(
    in_name: Option<&str>,
    out_name: Option<&str>,
    aec: bool,
    ns: bool,
    agc: bool,
    capture_ring: &Arc<CaptureRing>,
    playback_buf: &Arc<PlaybackBuffer>,
) -> Option<Box<dyn HwAudioDuplexHandle>> {
    let cap_ring = Arc::clone(capture_ring);
    let play_buf = Arc::clone(playback_buf);
    let on_capture = Arc::new(move |samples: &[f32]| {
        cap_ring.push(samples);
    });
    let on_render = Arc::new(move |out: &mut [f32]| {
        play_buf.pull_into(out);
    });
    let cfg = HwAudioDuplexConfig {
        sample_rate: SAMPLE_RATE,
        input_device: in_name,
        output_device: out_name,
        enable_aec_ns: aec || ns,
        enable_agc: agc,
    };
    match platform_backend().start_hw_audio_duplex(cfg, on_capture, on_render)? {
        Ok(hw) => Some(hw),
        Err(err) => {
            tracing::warn!(%err, "hardware audio DSP unavailable; falling back to cpal + sonora");
            None
        }
    }
}

#[inline]
fn push_mono_to_capture_ring(
    mono: &[f32],
    src_rate: u32,
    resampled_scratch: &mut Vec<f32>,
    capture_ring: &CaptureRing,
) {
    if src_rate == SAMPLE_RATE {
        capture_ring.push(mono);
    } else {
        resample_linear_into(mono, src_rate, SAMPLE_RATE, resampled_scratch);
        capture_ring.push(resampled_scratch);
    }
}

pub(super) fn open_input_stream(
    name: Option<&str>,
    capture_ring: Arc<CaptureRing>,
) -> Result<Stream> {
    let device = select_input_device(name)?;
    let supported = device
        .default_input_config()
        .map_err(|e| HuddleError::Audio(e.to_string()))?;
    let src_rate = supported.sample_rate();
    let channels = (supported.channels() as usize).max(1);
    let fmt = supported.sample_format();
    let config: StreamConfig = supported.into();

    let mut mono_scratch = Vec::with_capacity(RT_SCRATCH_SAMPLES);
    let mut resampled_scratch = Vec::with_capacity(RT_SCRATCH_SAMPLES);

    let stream = match fmt {
        SampleFormat::F32 => device.build_input_stream(
            config,
            move |data: &[f32], _| {
                downmix_to_mono_f32_into(data, channels, &mut mono_scratch);
                push_mono_to_capture_ring(
                    &mono_scratch,
                    src_rate,
                    &mut resampled_scratch,
                    &capture_ring,
                );
            },
            |_| {},
            None,
        ),
        SampleFormat::I16 => device.build_input_stream(
            config,
            move |data: &[i16], _| {
                mono_scratch.clear();
                let ch_div = channels as f32;
                for ch in data.chunks(channels) {
                    let sum: f32 = ch.iter().map(|&s| f32::from(s) / 32_768.0).sum();
                    mono_scratch.push(sum / ch_div);
                }
                push_mono_to_capture_ring(
                    &mono_scratch,
                    src_rate,
                    &mut resampled_scratch,
                    &capture_ring,
                );
            },
            |_| {},
            None,
        ),
        SampleFormat::U16 => device.build_input_stream(
            config,
            move |data: &[u16], _| {
                mono_scratch.clear();
                let ch_div = channels as f32;
                for ch in data.chunks(channels) {
                    let sum: f32 = ch
                        .iter()
                        .map(|&s| (f32::from(s) - 32_768.0) / 32_768.0)
                        .sum();
                    mono_scratch.push(sum / ch_div);
                }
                push_mono_to_capture_ring(
                    &mono_scratch,
                    src_rate,
                    &mut resampled_scratch,
                    &capture_ring,
                );
            },
            |_| {},
            None,
        ),
        SampleFormat::I32 => device.build_input_stream(
            config,
            move |data: &[i32], _| {
                mono_scratch.clear();
                let ch_div = channels as f32;
                let inv = 1.0 / (i32::MAX as f64);
                for ch in data.chunks(channels) {
                    let sum: f32 = ch.iter().map(|&s| (f64::from(s) * inv) as f32).sum();
                    mono_scratch.push(sum / ch_div);
                }
                push_mono_to_capture_ring(
                    &mono_scratch,
                    src_rate,
                    &mut resampled_scratch,
                    &capture_ring,
                );
            },
            |_| {},
            None,
        ),
        SampleFormat::F64 => device.build_input_stream(
            config,
            move |data: &[f64], _| {
                mono_scratch.clear();
                let ch_div = channels as f32;
                for ch in data.chunks(channels) {
                    let sum: f32 = ch.iter().map(|&s| s as f32).sum();
                    mono_scratch.push(sum / ch_div);
                }
                push_mono_to_capture_ring(
                    &mono_scratch,
                    src_rate,
                    &mut resampled_scratch,
                    &capture_ring,
                );
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

/// Build a `cpal` output stream supporting `F32`, `I16`, `U16`, `I32`, and `F64` host formats
/// while feeding the caller an interleaved `&mut [f32]` buffer of whole frames with zero
/// per-callback allocations.
pub(super) fn build_output_stream_any_format<F>(
    device: &cpal::Device,
    supported: SupportedStreamConfig,
    mut fill_f32: F,
) -> Result<Stream>
where
    F: FnMut(&mut [f32]) + Send + 'static,
{
    let fmt = supported.sample_format();
    let channels = (supported.channels() as usize).max(1);
    let chunk_elems = ((RT_SCRATCH_SAMPLES / channels).max(1)) * channels;
    let config: StreamConfig = supported.into();

    let stream = match fmt {
        SampleFormat::F32 => device.build_output_stream(
            config,
            move |data: &mut [f32], _| fill_f32(data),
            |_| {},
            None,
        ),
        SampleFormat::I16 => {
            let mut scratch = vec![0.0f32; chunk_elems];
            device.build_output_stream(
                config,
                move |data: &mut [i16], _| {
                    for chunk in data.chunks_mut(chunk_elems) {
                        let buf = &mut scratch[..chunk.len()];
                        fill_f32(buf);
                        for (dst, &src) in chunk.iter_mut().zip(buf.iter()) {
                            *dst = (src.clamp(-1.0, 1.0) * 32_767.0) as i16;
                        }
                    }
                },
                |_| {},
                None,
            )
        }
        SampleFormat::U16 => {
            let mut scratch = vec![0.0f32; chunk_elems];
            device.build_output_stream(
                config,
                move |data: &mut [u16], _| {
                    for chunk in data.chunks_mut(chunk_elems) {
                        let buf = &mut scratch[..chunk.len()];
                        fill_f32(buf);
                        for (dst, &src) in chunk.iter_mut().zip(buf.iter()) {
                            *dst = ((src.clamp(-1.0, 1.0) * 0.5 + 0.5) * 65_535.0) as u16;
                        }
                    }
                },
                |_| {},
                None,
            )
        }
        SampleFormat::I32 => {
            let mut scratch = vec![0.0f32; chunk_elems];
            device.build_output_stream(
                config,
                move |data: &mut [i32], _| {
                    for chunk in data.chunks_mut(chunk_elems) {
                        let buf = &mut scratch[..chunk.len()];
                        fill_f32(buf);
                        for (dst, &src) in chunk.iter_mut().zip(buf.iter()) {
                            *dst = (f64::from(src.clamp(-1.0, 1.0)) * f64::from(i32::MAX)) as i32;
                        }
                    }
                },
                |_| {},
                None,
            )
        }
        SampleFormat::F64 => {
            let mut scratch = vec![0.0f32; chunk_elems];
            device.build_output_stream(
                config,
                move |data: &mut [f64], _| {
                    for chunk in data.chunks_mut(chunk_elems) {
                        let buf = &mut scratch[..chunk.len()];
                        fill_f32(buf);
                        for (dst, &src) in chunk.iter_mut().zip(buf.iter()) {
                            *dst = f64::from(src.clamp(-1.0, 1.0));
                        }
                    }
                },
                |_| {},
                None,
            )
        }
        other => {
            return Err(HuddleError::Audio(format!(
                "unsupported output sample format {other:?}"
            )));
        }
    }
    .map_err(|e| HuddleError::Audio(e.to_string()))?;

    Ok(stream)
}

pub(super) fn open_output_stream(
    name: Option<&str>,
    playback_buf: Arc<PlaybackBuffer>,
) -> Result<Stream> {
    let device = select_output_device(name)?;
    let supported = device
        .default_output_config()
        .map_err(|e| HuddleError::Audio(e.to_string()))?;
    let dst_rate = supported.sample_rate();
    let channels = (supported.channels() as usize).max(1);

    let mut src_scratch = vec![0.0f32; RT_SCRATCH_SAMPLES];
    let mut resampled_scratch = Vec::with_capacity(RT_SCRATCH_SAMPLES);

    let stream = build_output_stream_any_format(&device, supported, move |data: &mut [f32]| {
        let out_frames = data.len() / channels;
        let needed_48k = ((out_frames as u64 * SAMPLE_RATE as u64).div_ceil(dst_rate.max(1) as u64)
            as usize)
            .min(src_scratch.len());
        playback_buf.pull_into(&mut src_scratch[..needed_48k]);
        let samples: &[f32] = if dst_rate == SAMPLE_RATE {
            &src_scratch[..needed_48k]
        } else {
            resample_linear_into(
                &src_scratch[..needed_48k],
                SAMPLE_RATE,
                dst_rate,
                &mut resampled_scratch,
            );
            &resampled_scratch
        };
        for (i, frame) in data.chunks_mut(channels).enumerate() {
            let s = samples.get(i).copied().unwrap_or(0.0).clamp(-1.0, 1.0);
            for ch in frame.iter_mut() {
                *ch = s;
            }
        }
    })?;

    stream
        .play()
        .map_err(|e| HuddleError::Audio(e.to_string()))?;
    Ok(stream)
}
