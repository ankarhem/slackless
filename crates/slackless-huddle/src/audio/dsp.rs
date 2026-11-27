use rubato::{FftFixedInOut, Resampler as _};
use sonora::config::{Config as SonoraConfig, EchoCanceller, GainController2, NoiseSuppression};
use sonora::{AudioProcessing, StreamConfig as SonoraStreamConfig};

use super::{FRAME_20MS, SAMPLE_RATE};
use crate::{HuddleError, Result};

/// Real-time generalized audio loudness normalizer (voice-activity-gated RMS leveler + soft-knee
/// peak limiter).
///
/// Keeps both remote speakers' voices and local microphone output at a consistent, comfortable
/// conversational loudness (`-18 dBFS` target RMS) without boosting background silence or clipping
/// loud transients.
#[derive(Debug, Clone)]
pub struct LoudnessNormalizer {
    target_rms: f32,
    gate_rms: f32,
    min_gain: f32,
    max_gain: f32,
    attack: f32,
    release: f32,
    current_gain: f32,
}

impl LoudnessNormalizer {
    /// Speech-tuned leveler (`-18 dBFS` target, `-44 dBFS` silence gate, fast ~80ms attack
    /// against shouting, gentle ~800ms release to lift quiet mics cleanly). The smoothing
    /// constants are per 20 ms of audio.
    pub fn speech() -> Self {
        Self {
            target_rms: 0.125, // ~ -18 dBFS
            gate_rms: 0.006,   // ~ -44 dBFS (ignores room noise / pauses)
            min_gain: 0.25,    // -12 dB max attenuation
            max_gain: 4.0,     // +12 dB max boost
            attack: 0.24,
            release: 0.03,
            current_gain: 1.0,
        }
    }

    /// Smoothly normalize one mono PCM frame in-place with soft-knee peak limiting. Frames of
    /// any length level at the same speed (10 ms playout frames, 20 ms capture frames).
    pub fn process_frame(&mut self, samples: &mut [f32]) {
        if samples.is_empty() {
            return;
        }
        let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
        let rms = (sum_sq / samples.len() as f32).sqrt();
        // Rescale a per-20 ms smoothing constant to this frame's length.
        let frames_20ms = samples.len() as f32 / FRAME_20MS as f32;
        let per_frame = |alpha: f32| 1.0 - (1.0 - alpha).powf(frames_20ms);

        let prev_gain = self.current_gain;
        if rms > self.gate_rms {
            let desired = (self.target_rms / rms).clamp(self.min_gain, self.max_gain);
            let alpha = if desired < self.current_gain {
                self.attack
            } else {
                self.release
            };
            self.current_gain += (desired - self.current_gain) * per_frame(alpha);
        } else {
            // Gently drift back toward unity during extended silence so the next utterance starts
            // cleanly without an aggressive residual boost.
            self.current_gain += (1.0 - self.current_gain) * per_frame(0.005);
        }

        let next_gain = self.current_gain;
        let denom = samples.len().max(1) as f32;
        for (i, s) in samples.iter_mut().enumerate() {
            // Interpolate gain across the frame to prevent zipper noise.
            let g = prev_gain + (next_gain - prev_gain) * (i as f32 / denom);
            *s = soft_limit_sample(*s * g);
        }
    }

    #[cfg(test)]
    pub(super) fn gain(&self) -> f32 {
        self.current_gain
    }
}

#[inline]
pub(super) fn soft_limit_sample(x: f32) -> f32 {
    const KNEE: f32 = 0.80;
    const HEADROOM: f32 = 0.18;
    let abs_x = x.abs();
    if abs_x <= KNEE {
        x
    } else {
        let excess = (abs_x - KNEE) / HEADROOM;
        let limited = KNEE + HEADROOM * excess.tanh();
        x.signum() * limited.min(0.98)
    }
}

/// Compute a bounded playback volume scaling factor to match `target_rms` given a file's measured
/// active RMS loudness.
pub fn normalized_gain_for_rms(rms: f32, target_rms: f32, min_gain: f32, max_gain: f32) -> f32 {
    if !rms.is_finite() || rms <= 1e-5 {
        return target_rms.clamp(min_gain, max_gain);
    }
    (target_rms / rms).clamp(min_gain, max_gain)
}

pub(super) fn build_sonora(aec: bool, ns: bool, agc: bool) -> AudioProcessing {
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

pub(super) fn downmix_to_mono_f32_into(data: &[f32], channels: usize, out: &mut Vec<f32>) {
    out.clear();
    if channels <= 1 {
        out.extend_from_slice(data);
        return;
    }
    let div = channels as f32;
    out.reserve(data.len() / channels);
    for ch in data.chunks(channels) {
        out.push(ch.iter().copied().sum::<f32>() / div);
    }
}

#[cfg(test)]
pub(super) fn downmix_to_mono_f32(data: &[f32], channels: usize) -> Vec<f32> {
    let mut out = Vec::new();
    downmix_to_mono_f32_into(data, channels, &mut out);
    out
}

/// Band-limited mono sample-rate converter: overlap-add FFT resampling, which low-pass filters
/// every block in the frequency domain, so decimation cannot fold content above the target
/// Nyquist into the speech band and interpolation adds no imaging.
///
/// [`Self::process`] never allocates: input is consumed in fixed `chunk_in`-frame blocks, each
/// resampled into a scratch block of `chunk_out` frames. The filter is one block long, so a
/// larger block means a steeper filter and more delay (`chunk_out / 2` frames).
pub(crate) struct RateConverter {
    resampler: FftFixedInOut<f32>,
    chunk_in: usize,
    chunk_out: usize,
    block: Vec<f32>,
    /// Leading input that does not complete a block yet; always shorter than `chunk_in`.
    pending: Vec<f32>,
}

impl RateConverter {
    /// Converter for one channel; `chunk_in` is the wanted input block length in frames (10 ms
    /// keeps real-time latency low, 100 ms suits offline media).
    pub(crate) fn new(src_rate: u32, dst_rate: u32, chunk_in: usize) -> Result<Self> {
        if chunk_in == 0
            || !(8_000..=384_000).contains(&src_rate)
            || !(8_000..=384_000).contains(&dst_rate)
        {
            return Err(HuddleError::Audio(format!(
                "unsupported audio sample rate or chunk ({src_rate} Hz -> {dst_rate} Hz, chunk {chunk_in})"
            )));
        }
        let resampler = FftFixedInOut::new(src_rate as usize, dst_rate as usize, chunk_in, 1)
            .map_err(|e| {
                HuddleError::Audio(format!(
                    "{src_rate} Hz to {dst_rate} Hz resampler unavailable: {e}"
                ))
            })?;
        let chunk_in = resampler.input_frames_next();
        let chunk_out = resampler.output_frames_max();
        Ok(Self {
            resampler,
            chunk_in,
            chunk_out,
            block: vec![0.0; chunk_out],
            pending: Vec::with_capacity(chunk_in),
        })
    }

    /// Input frames per block: feeding exactly this many keeps the output rate on ratio.
    pub(crate) fn chunk_in(&self) -> usize {
        self.chunk_in
    }

    /// Output frames per block; also the converter's delay.
    pub(crate) fn chunk_out(&self) -> usize {
        self.chunk_out
    }

    /// Most frames [`Self::process`] appends for an input of `input_frames` frames.
    pub(crate) fn output_capacity(&self, input_frames: usize) -> usize {
        (input_frames / self.chunk_in + 1) * self.chunk_out
    }

    /// Append `input` resampled to the target rate to `out`. Allocates only if `out` is shorter
    /// than [`Self::output_capacity`] for `input.len()`.
    pub(crate) fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let Self {
            resampler,
            chunk_in,
            block,
            pending,
            ..
        } = self;
        let mut input = input;
        if !pending.is_empty() {
            let take = (*chunk_in - pending.len()).min(input.len());
            pending.extend_from_slice(&input[..take]);
            input = &input[take..];
            if pending.len() == *chunk_in {
                resample_block(resampler, pending, block, out);
                pending.clear();
            }
        }
        let mut blocks = input.chunks_exact(*chunk_in);
        for block_in in blocks.by_ref() {
            resample_block(resampler, block_in, block, out);
        }
        pending.extend_from_slice(blocks.remainder());
    }
}

fn resample_block(
    resampler: &mut FftFixedInOut<f32>,
    input: &[f32],
    block: &mut [f32],
    out: &mut Vec<f32>,
) {
    // Both block lengths are fixed, so rubato's buffer validation cannot fail; should it ever,
    // skipping the block keeps a real-time callback allocation- and panic-free.
    if resampler
        .process_into_buffer(&[input], &mut [&mut block[..]], None)
        .is_err()
    {
        return;
    }
    out.extend_from_slice(block);
}

pub fn compute_rms_mono(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt().clamp(0.0, 1.0)
}

/// Zero-allocation interleaved RMS computation safe for real-time audio callbacks.
pub(super) fn compute_rms_interleaved(samples: &[f32], channels: usize) -> f32 {
    let ch = channels.max(1);
    if samples.len() < ch {
        return 0.0;
    }
    let inv_ch = 1.0 / ch as f32;
    let mut sum_sq = 0.0f32;
    let mut frames = 0usize;
    for frame in samples.chunks_exact(ch) {
        let mono = frame.iter().sum::<f32>() * inv_ch;
        sum_sq += mono * mono;
        frames += 1;
    }
    if frames == 0 {
        return 0.0;
    }
    (sum_sq / frames as f32).sqrt().clamp(0.0, 1.0)
}
