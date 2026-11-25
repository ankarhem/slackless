use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

use cpal::traits::{DeviceTrait as _, StreamTrait as _};
use cpal::{SampleFormat, Stream, StreamConfig};
use sonora::AudioProcessing;

use super::devices::{select_input_device, select_output_device};
use super::dsp::{downmix_to_mono_f32_into, resample_linear_into};
use super::{FRAME_20MS, PlaybackBuffer, SAMPLE_RATE};
use crate::backend::{HwAudioDuplexHandle, platform_backend};
use crate::{HuddleError, Result};

pub(super) type CaptureSignal = Arc<(Mutex<VecDeque<f32>>, Condvar)>;

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
    capture_signal: &CaptureSignal,
    playback_buf: &Arc<Mutex<PlaybackBuffer>>,
) -> Option<Box<dyn HwAudioDuplexHandle>> {
    let cap_sig = Arc::clone(capture_signal);
    let play_buf = Arc::clone(playback_buf);
    let on_capture = Arc::new(move |samples: &[f32]| {
        let (lock, cvar) = &*cap_sig;
        if let Ok(mut q) = lock.lock() {
            if q.len() > FRAME_20MS * 8 {
                let drop_n = q.len() - FRAME_20MS * 4;
                q.drain(..drop_n);
            }
            q.extend(samples.iter().copied());
            if q.len() >= FRAME_20MS {
                cvar.notify_one();
            }
        }
    });
    let on_render = Arc::new(move |out: &mut [f32]| {
        // Real-time CoreAudio callback: use `try_lock` so we never priority-invert on the
        // decoder thread; lock hold time on `push` is a few hundred nanoseconds.
        if let Ok(mut pb) = play_buf.try_lock() {
            pb.pull_into(out);
        } else {
            out.fill(0.0);
        }
    });
    match platform_backend().start_hw_audio_duplex(
        SAMPLE_RATE,
        in_name,
        out_name,
        aec || ns,
        agc,
        on_capture,
        on_render,
    )? {
        Ok(hw) => Some(hw),
        Err(err) => {
            tracing::warn!(%err, "hardware audio DSP unavailable; falling back to cpal + sonora");
            None
        }
    }
}

pub(super) fn open_input_stream(name: Option<&str>, signal: CaptureSignal) -> Result<Stream> {
    let device = select_input_device(name)?;
    let supported = device
        .default_input_config()
        .map_err(|e| HuddleError::Audio(e.to_string()))?;
    let src_rate = supported.sample_rate();
    let channels = supported.channels() as usize;
    let fmt = supported.sample_format();
    let config: StreamConfig = supported.into();

    let mut mono_scratch = Vec::with_capacity(FRAME_20MS * 2);
    let mut resampled_scratch = Vec::with_capacity(FRAME_20MS * 2);

    let stream = match fmt {
        SampleFormat::F32 => device.build_input_stream(
            config,
            move |data: &[f32], _| {
                downmix_to_mono_f32_into(data, channels, &mut mono_scratch);
                let samples: &[f32] = if src_rate == SAMPLE_RATE {
                    &mono_scratch
                } else {
                    resample_linear_into(
                        &mono_scratch,
                        src_rate,
                        SAMPLE_RATE,
                        &mut resampled_scratch,
                    );
                    &resampled_scratch
                };
                let (lock, cvar) = &*signal;
                if let Ok(mut q) = lock.lock() {
                    if q.len() > FRAME_20MS * 8 {
                        let drop_n = q.len() - FRAME_20MS * 4;
                        q.drain(..drop_n);
                    }
                    q.extend(samples.iter().copied());
                    if q.len() >= FRAME_20MS {
                        cvar.notify_one();
                    }
                }
            },
            |_| {},
            None,
        ),
        SampleFormat::I16 => device.build_input_stream(
            config,
            move |data: &[i16], _| {
                mono_scratch.clear();
                let ch_div = channels.max(1) as f32;
                for ch in data.chunks(channels.max(1)) {
                    let sum: f32 = ch.iter().map(|&s| s as f32 / 32768.0).sum();
                    mono_scratch.push(sum / ch_div);
                }
                let samples: &[f32] = if src_rate == SAMPLE_RATE {
                    &mono_scratch
                } else {
                    resample_linear_into(
                        &mono_scratch,
                        src_rate,
                        SAMPLE_RATE,
                        &mut resampled_scratch,
                    );
                    &resampled_scratch
                };
                let (lock, cvar) = &*signal;
                if let Ok(mut q) = lock.lock() {
                    if q.len() > FRAME_20MS * 8 {
                        let drop_n = q.len() - FRAME_20MS * 4;
                        q.drain(..drop_n);
                    }
                    q.extend(samples.iter().copied());
                    if q.len() >= FRAME_20MS {
                        cvar.notify_one();
                    }
                }
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

pub(super) fn open_output_stream(
    name: Option<&str>,
    playback_buf: Arc<Mutex<PlaybackBuffer>>,
) -> Result<Stream> {
    let device = select_output_device(name)?;
    let supported = device
        .default_output_config()
        .map_err(|e| HuddleError::Audio(e.to_string()))?;
    let dst_rate = supported.sample_rate();
    let channels = supported.channels() as usize;
    let config: StreamConfig = supported.into();

    let mut src_scratch = Vec::with_capacity(FRAME_20MS * 2);
    let mut resampled_scratch = Vec::with_capacity(FRAME_20MS * 2);

    let stream = device
        .build_output_stream(
            config,
            move |data: &mut [f32], _| {
                let out_frames = data.len() / channels.max(1);
                let needed_48k = ((out_frames as u64 * SAMPLE_RATE as u64)
                    .div_ceil(dst_rate.max(1) as u64)) as usize;
                src_scratch.resize(needed_48k, 0.0);
                if let Ok(mut pb) = playback_buf.try_lock() {
                    pb.pull_into(&mut src_scratch[..needed_48k]);
                } else {
                    src_scratch[..needed_48k].fill(0.0);
                }
                let samples: &[f32] = if dst_rate == SAMPLE_RATE {
                    &src_scratch
                } else {
                    resample_linear_into(
                        &src_scratch,
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
