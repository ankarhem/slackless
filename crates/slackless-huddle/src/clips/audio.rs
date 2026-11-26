//! Audio clip recording, resampling, WAV encoding, waveform downsampling, and AAC transcoding.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _};
use sonora::config::{Config as SonoraConfig, GainController2, NoiseSuppression};
use sonora::{AudioProcessing, StreamConfig as SonoraStreamConfig};

use crate::audio::ring::SpscRing;
use crate::audio::{FRAME_10MS, RateConverter, SAMPLE_RATE, build_input_stream_any_format};
use crate::{HuddleError, Result};

use super::{LIVE_BARS, MAX_AUDIO_SAMPLES, RecordedClip, timestamp_slug};

#[cfg(target_os = "macos")]
static AFCONVERT_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Live microphone clip recorder backed by `cpal` + lock-free SPSC ring + worker-thread `sonora`
/// DSP (`F-CORE-127`).
pub struct AudioClipRecorder {
    stream: Option<cpal::Stream>,
    running: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    samples: Arc<Mutex<Vec<i16>>>,
    bars: Arc<Mutex<VecDeque<f32>>>,
    started_at: Instant,
}

/// Push downmixed mono samples into `ring` using a fixed stack buffer (zero heap allocations
/// and zero locks in the real-time `cpal` callback).
fn push_downmixed_rt(data: &[f32], channels: usize, ring: &SpscRing<f32>) {
    let div = channels as f32;
    let mut stack = [0.0f32; 512];
    let mut n = 0usize;
    for frame in data.chunks(channels) {
        stack[n] = frame.iter().sum::<f32>() / div;
        n += 1;
        if n == stack.len() {
            let _ = ring.push_slice(&stack);
            n = 0;
        }
    }
    if n > 0 {
        let _ = ring.push_slice(&stack[..n]);
    }
}

impl AudioClipRecorder {
    /// Start capturing audio from the default input microphone.
    pub fn start() -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| HuddleError::Audio("no input microphone available".into()))?;
        let supported = device
            .default_input_config()
            .map_err(|e| HuddleError::Audio(e.to_string()))?;
        let in_rate = supported.sample_rate();
        let in_channels = usize::from(supported.channels()).max(1);
        // 10 ms blocks, as in calls: the filter edge sits far above speech either way, and a
        // stopped recording loses at most one block plus the 5 ms converter delay.
        let mut converter = (in_rate != SAMPLE_RATE)
            .then(|| RateConverter::new(in_rate, SAMPLE_RATE, in_rate as usize / 100))
            .transpose()?;

        let samples = Arc::new(Mutex::new(Vec::<i16>::with_capacity(
            (SAMPLE_RATE as usize) * 15,
        )));
        let bars = Arc::new(Mutex::new(VecDeque::from(vec![0.0f32; LIVE_BARS])));
        let running = Arc::new(AtomicBool::new(true));
        // Lock-free SPSC ring (~1.3s at 48 kHz mono) decouples the real-time cpal callback from
        // Sonora DSP and sample accumulation.
        let ring = Arc::new(SpscRing::<f32>::new(65_536));

        let ring_cb = Arc::clone(&ring);
        let stream = build_input_stream_any_format(&device, supported, move |data| {
            push_downmixed_rt(data, in_channels, &ring_cb);
        })?;

        let samples_worker = Arc::clone(&samples);
        let bars_worker = Arc::clone(&bars);
        let run_worker = Arc::clone(&running);
        let ring_worker = Arc::clone(&ring);

        let worker = std::thread::Builder::new()
            .name("slackless-clip-audio-dsp".to_owned())
            .spawn(move || {
                let apm_cfg = SonoraConfig {
                    noise_suppression: Some(NoiseSuppression::default()),
                    gain_controller2: Some(GainController2::default()),
                    ..Default::default()
                };
                let apm_stream = SonoraStreamConfig::new(SAMPLE_RATE, 1);
                let mut apm = AudioProcessing::builder()
                    .config(apm_cfg)
                    .capture_config(apm_stream)
                    .build();
                let mut apm_pending: Vec<f32> = Vec::with_capacity(FRAME_10MS * 4);
                let mut raw_buf = [0.0f32; 2048];
                let mut resampled = Vec::with_capacity(
                    converter
                        .as_ref()
                        .map_or(FRAME_10MS * 4, |c| c.output_capacity(raw_buf.len())),
                );
                let mut processed_i16: Vec<i16> = Vec::with_capacity(FRAME_10MS * 4);
                let mut bar_accum = 0.0f32;
                let mut bar_chunks = 0usize;

                let mut process_mono = |mono: &[f32]| {
                    resampled.clear();
                    match &mut converter {
                        Some(conv) => conv.process(mono, &mut resampled),
                        None => resampled.extend_from_slice(mono),
                    }
                    apm_pending.extend_from_slice(&resampled);
                    processed_i16.clear();
                    let mut sum_sq = 0.0f32;
                    let mut count = 0usize;
                    while apm_pending.len() >= FRAME_10MS {
                        let mut frame: [f32; FRAME_10MS] = [0.0; FRAME_10MS];
                        frame.copy_from_slice(&apm_pending[..FRAME_10MS]);
                        apm_pending.drain(..FRAME_10MS);
                        let mut out = [0.0f32; FRAME_10MS];
                        let src = [&frame[..]];
                        let mut dst = [&mut out[..]];
                        if apm.process_capture_f32(&src, &mut dst).is_err() {
                            out = frame;
                        }
                        for &s in &out {
                            let clamped = s.clamp(-1.0, 1.0);
                            sum_sq += clamped * clamped;
                            count += 1;
                            processed_i16.push((clamped * 32767.0) as i16);
                        }
                    }
                    if count > 0 {
                        let rms = (sum_sq / count as f32).sqrt().min(1.0);
                        let visual = (rms * 3.2).min(1.0);
                        bar_accum = bar_accum.max(visual);
                        bar_chunks += 1;
                        if bar_chunks >= 4 {
                            if let Ok(mut q) = bars_worker.lock() {
                                if q.len() >= LIVE_BARS {
                                    q.pop_front();
                                }
                                q.push_back(bar_accum);
                            }
                            bar_accum = 0.0;
                            bar_chunks = 0;
                        }
                    }
                    if !processed_i16.is_empty()
                        && let Ok(mut buf) = samples_worker.lock()
                    {
                        let remaining = MAX_AUDIO_SAMPLES.saturating_sub(buf.len());
                        if remaining > 0 {
                            let take = processed_i16.len().min(remaining);
                            buf.extend_from_slice(&processed_i16[..take]);
                        }
                    }
                };

                loop {
                    let got = ring_worker.pop_slice(&mut raw_buf);
                    if got > 0 {
                        process_mono(&raw_buf[..got]);
                    } else if !run_worker.load(Ordering::Acquire) {
                        break;
                    } else {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
            })
            .map_err(|e| HuddleError::Audio(e.to_string()))?;

        if let Err(e) = stream.play() {
            drop(stream);
            running.store(false, Ordering::Release);
            let _ = worker.join();
            return Err(HuddleError::Audio(e.to_string()));
        }

        Ok(Self {
            stream: Some(stream),
            running,
            worker: Some(worker),
            samples,
            bars,
            started_at: Instant::now(),
        })
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }

    pub fn live_bars(&self) -> Vec<f32> {
        self.bars
            .lock()
            .map(|q| q.iter().copied().collect())
            .unwrap_or_else(|_| vec![0.0; LIVE_BARS])
    }

    fn shutdown(&mut self) {
        // Drop stream first so no more callbacks push to the ring, then drain and join worker.
        drop(self.stream.take());
        self.running.store(false, Ordering::Release);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }

    /// Discard the recording without encoding.
    pub fn cancel(mut self) {
        self.shutdown();
    }

    /// Stop capturing on the caller's thread and return the raw 48 kHz mono PCM buffer + elapsed
    /// duration so encoding/transcoding can run off the UI thread via [`finish_audio_clip`].
    pub fn stop_to_pcm(mut self) -> Result<(Vec<i16>, u64)> {
        let elapsed_ms = self.elapsed_ms();
        self.shutdown();
        let pcm = self
            .samples
            .lock()
            .map(|g| g.clone())
            .map_err(|_| HuddleError::Audio("audio clip buffer poisoned".into()))?;
        Ok((pcm, elapsed_ms))
    }

    /// Stop recording and encode the clip to AAC `.m4a` (macOS `afconvert`) or PCM `.wav`.
    pub fn stop(self) -> Result<RecordedClip> {
        let (pcm, elapsed_ms) = self.stop_to_pcm()?;
        finish_audio_clip(pcm, elapsed_ms)
    }
}

impl Drop for AudioClipRecorder {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Encode captured 48 kHz mono PCM into an AAC `.m4a` (macOS) or `.wav` [`RecordedClip`].
pub fn finish_audio_clip(pcm: Vec<i16>, elapsed_ms: u64) -> Result<RecordedClip> {
    if pcm.is_empty() {
        return Err(HuddleError::Audio("no audio captured".into()));
    }
    let duration_ms = ((pcm.len() as u64) * 1000 / SAMPLE_RATE as u64).max(elapsed_ms.min(500));
    let waveform = downsample_waveform(&pcm, 32);
    let wav = encode_wav_pcm16(&pcm, SAMPLE_RATE, 1);
    let slug = timestamp_slug();

    if let Some(m4a) = transcode_wav_to_m4a(&wav) {
        return Ok(RecordedClip {
            filename: format!("audio-clip-{slug}.m4a"),
            mime: "audio/mp4".to_owned(),
            bytes: m4a,
            duration_ms,
            waveform,
            preview_frame: None,
            thumbnail_frames: Vec::new(),
        });
    }

    Ok(RecordedClip {
        filename: format!("audio-clip-{slug}.wav"),
        mime: "audio/wav".to_owned(),
        bytes: wav,
        duration_ms,
        waveform,
        preview_frame: None,
        thumbnail_frames: Vec::new(),
    })
}

/// Encode signed 16-bit PCM samples into a standard RIFF/WAVE file.
pub fn encode_wav_pcm16(samples: &[i16], sample_rate: u32, channels: u16) -> Vec<u8> {
    let data_bytes = (samples.len() * 2) as u32;
    let byte_rate = sample_rate * channels as u32 * 2;
    let block_align = channels * 2;
    let mut out = Vec::with_capacity(44 + data_bytes as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36u32.saturating_add(data_bytes)).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    for &s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Downsample PCM samples into `bars` normalized amplitude bars (`0.0..=1.0`).
pub fn downsample_waveform(samples: &[i16], bars: usize) -> Vec<f32> {
    if bars == 0 || samples.is_empty() {
        return Vec::new();
    }
    let chunk = (samples.len() / bars).max(1);
    let mut peaks = Vec::with_capacity(bars);
    let mut max_peak = 0.01f32;
    for i in 0..bars {
        let start = (i * samples.len()) / bars;
        let end = (start + chunk).min(samples.len());
        let rms = if end > start {
            let sq: f32 = samples[start..end]
                .iter()
                .map(|&s| {
                    let f = s as f32 / 32768.0;
                    f * f
                })
                .sum();
            (sq / (end - start) as f32).sqrt()
        } else {
            0.0
        };
        max_peak = max_peak.max(rms);
        peaks.push(rms);
    }
    peaks
        .into_iter()
        .map(|p| (p / max_peak).clamp(0.08, 1.0))
        .collect()
}

/// Shared macOS `/usr/bin/afconvert` transcoder with atomic collision-free temp filenames.
#[cfg(target_os = "macos")]
fn transcode_wav_with_afconvert(wav: &[u8], file_format: &str, out_ext: &str) -> Option<Vec<u8>> {
    let dir = std::env::temp_dir();
    let stamp = timestamp_slug();
    let pid = std::process::id();
    let seq = AFCONVERT_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let wav_path = dir.join(format!("slackless-clip-{pid}-{stamp}-{seq}.wav"));
    let out_path = dir.join(format!("slackless-clip-{pid}-{stamp}-{seq}.{out_ext}"));
    std::fs::write(&wav_path, wav).ok()?;
    let status = std::process::Command::new("/usr/bin/afconvert")
        .args(["-f", file_format, "-d", "aac", "-b", "96000"])
        .arg(&wav_path)
        .arg(&out_path)
        .status()
        .ok()?;
    let _ = std::fs::remove_file(&wav_path);
    if !status.success() {
        let _ = std::fs::remove_file(&out_path);
        return None;
    }
    let bytes = std::fs::read(&out_path).ok();
    let _ = std::fs::remove_file(&out_path);
    bytes.filter(|b| !b.is_empty())
}

#[cfg(not(target_os = "macos"))]
fn transcode_wav_with_afconvert(
    _wav: &[u8],
    _file_format: &str,
    _out_ext: &str,
) -> Option<Vec<u8>> {
    None
}

fn transcode_wav_to_m4a(wav: &[u8]) -> Option<Vec<u8>> {
    transcode_wav_with_afconvert(wav, "m4af", "m4a")
}

pub(super) fn transcode_wav_to_aac_adts(wav: &[u8]) -> Option<Vec<u8>> {
    transcode_wav_with_afconvert(wav, "adts", "aac")
}

/// Decoded raw AAC-LC frames ready for ISO-BMFF `mp4a`/`esds` track muxing.
#[derive(Debug, Clone)]
pub struct AacAudioTrack {
    pub sample_rate: u32,
    pub channels: u16,
    pub audio_specific_config: [u8; 2],
    pub frames: Vec<Vec<u8>>,
}

const ADTS_SAMPLE_RATES: [u32; 13] = [
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000,
    7_350,
];

/// Parse an ADTS AAC bitstream into raw access units + 2-byte MPEG-4 `AudioSpecificConfig`.
pub fn parse_adts_frames(data: &[u8]) -> Option<AacAudioTrack> {
    let mut pos = 0usize;
    let mut frames = Vec::new();
    let mut header_meta: Option<(u32, u16, [u8; 2])> = None;

    while pos + 7 <= data.len() {
        if data[pos] != 0xFF || (data[pos + 1] & 0xF0) != 0xF0 {
            pos += 1;
            continue;
        }
        let protection_absent = (data[pos + 1] & 0x01) != 0;
        let header_len = if protection_absent { 7 } else { 9 };
        if pos + header_len > data.len() {
            break;
        }
        let profile_object_type = ((data[pos + 2] >> 6) & 0x03) + 1; // 2 = AAC-LC
        let sr_idx = ((data[pos + 2] >> 2) & 0x0F) as usize;
        let ch_cfg = ((data[pos + 2] & 0x01) << 2) | ((data[pos + 3] >> 6) & 0x03);
        let frame_len = (((data[pos + 3] & 0x03) as usize) << 11)
            | ((data[pos + 4] as usize) << 3)
            | (((data[pos + 5] >> 5) & 0x07) as usize);
        if frame_len <= header_len || pos + frame_len > data.len() {
            break;
        }
        if header_meta.is_none() {
            let sr = *ADTS_SAMPLE_RATES.get(sr_idx)?;
            let channels = (ch_cfg as u16).max(1);
            let asc0 = (profile_object_type << 3) | ((sr_idx as u8) >> 1);
            let asc1 = (((sr_idx as u8) & 0x01) << 7) | ((ch_cfg & 0x0F) << 3);
            header_meta = Some((sr, channels, [asc0, asc1]));
        }
        frames.push(data[pos + header_len..pos + frame_len].to_vec());
        pos += frame_len;
    }

    let (sample_rate, channels, audio_specific_config) = header_meta?;
    if frames.is_empty() {
        return None;
    }
    Some(AacAudioTrack {
        sample_rate,
        channels,
        audio_specific_config,
        frames,
    })
}
