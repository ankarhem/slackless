use sonora::config::{Config as SonoraConfig, EchoCanceller, GainController2, NoiseSuppression};
use sonora::{AudioProcessing, StreamConfig as SonoraStreamConfig};

use super::SAMPLE_RATE;

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
    /// Speech-tuned leveler for 20ms VoIP frames (`-18 dBFS` target, `-44 dBFS` silence gate,
    /// fast ~80ms attack against shouting, gentle ~800ms release to lift quiet mics cleanly).
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

    /// Smoothly normalize a 20ms mono PCM frame in-place with soft-knee peak limiting.
    pub fn process_frame(&mut self, samples: &mut [f32]) {
        if samples.is_empty() {
            return;
        }
        let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
        let rms = (sum_sq / samples.len() as f32).sqrt();

        let prev_gain = self.current_gain;
        if rms > self.gate_rms {
            let desired = (self.target_rms / rms).clamp(self.min_gain, self.max_gain);
            let alpha = if desired < self.current_gain {
                self.attack
            } else {
                self.release
            };
            self.current_gain += (desired - self.current_gain) * alpha;
        } else {
            // Gently drift back toward unity during extended silence so the next utterance starts
            // cleanly without an aggressive residual boost.
            self.current_gain += (1.0 - self.current_gain) * 0.005;
        }

        let next_gain = self.current_gain;
        let denom = samples.len().max(1) as f32;
        for (i, s) in samples.iter_mut().enumerate() {
            // Interpolate gain across the 20ms frame to prevent zipper noise.
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

pub(crate) fn resample_linear_into(
    input: &[f32],
    src_rate: u32,
    dst_rate: u32,
    out: &mut Vec<f32>,
) {
    out.clear();
    if src_rate == dst_rate || input.is_empty() {
        out.extend_from_slice(input);
        return;
    }
    let ratio = src_rate as f64 / dst_rate as f64;
    let out_len = ((input.len() as f64) / ratio).round().max(1.0) as usize;
    out.reserve(out_len);
    for i in 0..out_len {
        let pos = i as f64 * ratio;
        let idx = pos.floor() as usize;
        let frac = (pos - idx as f64) as f32;
        let s0 = input.get(idx).copied().unwrap_or(0.0);
        let s1 = input.get(idx + 1).copied().unwrap_or(s0);
        out.push(s0 + (s1 - s0) * frac);
    }
}

/// Linear resampler used when a macOS device runs at 44.1 kHz instead of 48 kHz.
pub fn resample_linear(input: &[f32], src_rate: u32, dst_rate: u32) -> Vec<f32> {
    let mut out = Vec::new();
    resample_linear_into(input, src_rate, dst_rate, &mut out);
    out
}

pub fn compute_rms_mono(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt().clamp(0.0, 1.0)
}

/// Zero-allocation interleaved RMS computation safe for real-time audio callbacks.
pub(super) fn compute_rms_interleaved_map<T: Copy>(
    samples: &[T],
    channels: usize,
    to_f32: impl Fn(T) -> f32,
) -> f32 {
    let ch = channels.max(1);
    if samples.len() < ch {
        return 0.0;
    }
    let inv_ch = 1.0 / ch as f32;
    let mut sum_sq = 0.0f32;
    let mut frames = 0usize;
    for frame in samples.chunks_exact(ch) {
        let mut mono = 0.0f32;
        for &s in frame {
            mono += to_f32(s);
        }
        mono *= inv_ch;
        sum_sq += mono * mono;
        frames += 1;
    }
    if frames == 0 {
        return 0.0;
    }
    (sum_sq / frames as f32).sqrt().clamp(0.0, 1.0)
}
