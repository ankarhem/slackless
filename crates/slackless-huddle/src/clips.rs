//! Native audio and video clip recording (`F-CORE-127`, `F-CORE-128`, `F-CORE-131`):
//! - [`AudioClipRecorder`]: captures microphone via `cpal` + `sonora` NS/AGC, computes live RMS
//!   bars, encodes PCM WAV (or compact AAC `.m4a` via macOS `afconvert` when available).
//! - [`VideoClipRecorder`]: captures webcam (`nokhwa`) or screen/window (`xcap`), encodes H.264
//!   via Cisco OpenH264, and muxes a valid ISO-BMFF `.mp4` (`ftyp`/`mdat`/`moov`/`avcC`) in pure
//!   Rust with zero external binary requirements.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cpal::SampleFormat;
use cpal::traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _};
use nokhwa::Camera;
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{CameraIndex, RequestedFormat, RequestedFormatType};
use sonora::config::{Config as SonoraConfig, GainController2, NoiseSuppression};
use sonora::{AudioProcessing, StreamConfig as SonoraStreamConfig};

use crate::audio::{FRAME_10MS, SAMPLE_RATE};
use crate::screen::{SCREEN_MAX_H, SCREEN_MAX_W};
use crate::video::{
    CAMERA_MAX_H, CAMERA_MAX_W, H264VideoEncoder, VideoFrame, resize_rgb_even, rgb_to_bgra,
    rgba_to_rgb_and_bgra,
};
use crate::{HuddleError, Result};

/// Maximum audio clip length: 5 minutes at 48 kHz mono.
const MAX_AUDIO_SAMPLES: usize = (SAMPLE_RATE as usize) * 60 * 5;
/// Maximum video clip frames (~3 minutes at 12 fps).
const MAX_VIDEO_FRAMES: usize = 12 * 180;
const LIVE_BARS: usize = 24;
const CLIP_FPS: u32 = 12;

/// Output of a finished audio or video clip recording, ready to attach and upload.
#[derive(Debug, Clone)]
pub struct RecordedClip {
    pub filename: String,
    pub mime: String,
    pub bytes: Vec<u8>,
    pub duration_ms: u64,
    pub waveform: Vec<f32>,
    pub preview_frame: Option<VideoFrame>,
}

/// Source for [`VideoClipRecorder`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipVideoSource {
    Camera(Option<usize>),
    Screen(String),
}

fn timestamp_slug() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Live microphone clip recorder backed by `cpal` + `sonora` DSP (`F-CORE-127`).
pub struct AudioClipRecorder {
    _stream: cpal::Stream,
    samples: Arc<Mutex<Vec<i16>>>,
    rms_bits: Arc<AtomicU32>,
    bars: Arc<Mutex<VecDeque<f32>>>,
    started_at: Instant,
}

// `cpal::Stream` is `!Send` on some platforms, so we keep `AudioClipRecorder` on the UI thread
// (or drop it where created) — GPUI entities live on the main thread!
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
        let sample_format = supported.sample_format();
        let in_rate = supported.sample_rate().0;
        let in_channels = supported.channels() as usize;
        let config: cpal::StreamConfig = supported.into();

        let samples = Arc::new(Mutex::new(Vec::<i16>::with_capacity(
            (SAMPLE_RATE as usize) * 15,
        )));
        let rms_bits = Arc::new(AtomicU32::new(0));
        let bars = Arc::new(Mutex::new(VecDeque::from(vec![0.0f32; LIVE_BARS])));

        let samples_cb = Arc::clone(&samples);
        let rms_cb = Arc::clone(&rms_bits);
        let bars_cb = Arc::clone(&bars);

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
        let mut apm_pending: Vec<f32> = Vec::with_capacity(FRAME_10MS * 2);
        let mut bar_accum = 0.0f32;
        let mut bar_chunks = 0usize;

        let mut push_mono_f32 = move |mono: &[f32]| {
            let resampled = resample_mono(mono, in_rate, SAMPLE_RATE);
            apm_pending.extend_from_slice(&resampled);
            let mut processed_i16 = Vec::new();
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
                // Boost visual sensitivity slightly so normal speech fills the meter nicely.
                let visual = (rms * 3.2).min(1.0);
                rms_cb.store(visual.to_bits(), Ordering::Relaxed);
                bar_accum = bar_accum.max(visual);
                bar_chunks += 1;
                if bar_chunks >= 4 {
                    if let Ok(mut q) = bars_cb.lock() {
                        if q.len() >= LIVE_BARS {
                            q.pop_front();
                        }
                        q.push_back(bar_accum);
                    }
                    bar_accum = 0.0;
                    bar_chunks = 0;
                }
            }
            if !processed_i16.is_empty() {
                if let Ok(mut buf) = samples_cb.lock() {
                    let remaining = MAX_AUDIO_SAMPLES.saturating_sub(buf.len());
                    if remaining > 0 {
                        let take = processed_i16.len().min(remaining);
                        buf.extend_from_slice(&processed_i16[..take]);
                    }
                }
            }
        };

        let err_fn = |err| {
            eprintln!("slackless: audio clip stream error: {err}");
        };

        let stream = match sample_format {
            SampleFormat::F32 => device
                .build_input_stream(
                    &config,
                    move |data: &[f32], _| {
                        let mono: Vec<f32> = data
                            .chunks(in_channels.max(1))
                            .map(|f| f.iter().copied().sum::<f32>() / f.len() as f32)
                            .collect();
                        push_mono_f32(&mono);
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| HuddleError::Audio(e.to_string()))?,
            SampleFormat::I16 => device
                .build_input_stream(
                    &config,
                    move |data: &[i16], _| {
                        let mono: Vec<f32> = data
                            .chunks(in_channels.max(1))
                            .map(|f| {
                                f.iter().map(|&s| s as f32 / 32768.0).sum::<f32>() / f.len() as f32
                            })
                            .collect();
                        push_mono_f32(&mono);
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| HuddleError::Audio(e.to_string()))?,
            SampleFormat::U16 => device
                .build_input_stream(
                    &config,
                    move |data: &[u16], _| {
                        let mono: Vec<f32> = data
                            .chunks(in_channels.max(1))
                            .map(|f| {
                                f.iter()
                                    .map(|&s| (s as f32 - 32768.0) / 32768.0)
                                    .sum::<f32>()
                                    / f.len() as f32
                            })
                            .collect();
                        push_mono_f32(&mono);
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| HuddleError::Audio(e.to_string()))?,
            other => {
                return Err(HuddleError::Audio(format!(
                    "unsupported input sample format: {other:?}"
                )));
            }
        };

        stream
            .play()
            .map_err(|e| HuddleError::Audio(e.to_string()))?;

        Ok(Self {
            _stream: stream,
            samples,
            rms_bits,
            bars,
            started_at: Instant::now(),
        })
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }

    pub fn current_rms(&self) -> f32 {
        f32::from_bits(self.rms_bits.load(Ordering::Relaxed))
    }

    pub fn live_bars(&self) -> Vec<f32> {
        self.bars
            .lock()
            .map(|q| q.iter().copied().collect())
            .unwrap_or_else(|_| vec![0.0; LIVE_BARS])
    }

    /// Discard the recording without encoding.
    pub fn cancel(self) {
        drop(self);
    }

    /// Stop capturing on the caller's thread and return the raw 48 kHz mono PCM buffer + elapsed
    /// duration so encoding/transcoding can run off the UI thread via [`finish_audio_clip`].
    pub fn stop_to_pcm(self) -> Result<(Vec<i16>, u64)> {
        let elapsed_ms = self.elapsed_ms();
        let samples = self.samples.clone();
        drop(self._stream);
        let pcm = samples
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
        });
    }

    Ok(RecordedClip {
        filename: format!("audio-clip-{slug}.wav"),
        mime: "audio/wav".to_owned(),
        bytes: wav,
        duration_ms,
        waveform,
        preview_frame: None,
    })
}

/// Live camera or screen video clip recorder (`F-CORE-128`) with synchronized microphone audio.
pub struct VideoClipRecorder {
    running: Arc<AtomicBool>,
    latest_preview: Arc<Mutex<Option<VideoFrame>>>,
    frames: Arc<Mutex<Vec<EncodedH264Frame>>>,
    audio_pcm: Arc<Mutex<Vec<i16>>>,
    dimensions: Arc<Mutex<(u32, u32)>>,
    worker: Option<std::thread::JoinHandle<()>>,
    audio_worker: Option<std::thread::JoinHandle<()>>,
    started_at: Instant,
}

struct EncodedH264Frame {
    annex_b: Vec<u8>,
}

impl VideoClipRecorder {
    /// Start capturing camera or screen frames into H.264 plus synchronized microphone audio.
    pub fn start(source: ClipVideoSource) -> Result<Self> {
        let running = Arc::new(AtomicBool::new(true));
        let latest_preview: Arc<Mutex<Option<VideoFrame>>> = Arc::new(Mutex::new(None));
        let frames: Arc<Mutex<Vec<EncodedH264Frame>>> =
            Arc::new(Mutex::new(Vec::with_capacity(120)));
        let audio_pcm: Arc<Mutex<Vec<i16>>> =
            Arc::new(Mutex::new(Vec::with_capacity((SAMPLE_RATE as usize) * 15)));
        let dimensions: Arc<Mutex<(u32, u32)>> = Arc::new(Mutex::new((0, 0)));

        let run_flag = Arc::clone(&running);
        let preview_cb = Arc::clone(&latest_preview);
        let frames_cb = Arc::clone(&frames);
        let dims_cb = Arc::clone(&dimensions);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<()>>(1);

        // Optional synchronized microphone capture thread (fails gracefully if no mic is available).
        let audio_run = Arc::clone(&running);
        let audio_buf = Arc::clone(&audio_pcm);
        let audio_worker = std::thread::Builder::new()
            .name("slackless-video-clip-audio".to_owned())
            .spawn(move || {
                let Ok(rec) = AudioClipRecorder::start() else {
                    return;
                };
                while audio_run.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(40));
                }
                if let Ok((pcm, _)) = rec.stop_to_pcm() {
                    if let Ok(mut dst) = audio_buf.lock() {
                        *dst = pcm;
                    }
                }
            })
            .ok();

        let worker = std::thread::Builder::new()
            .name("slackless-video-clip".to_owned())
            .spawn(move || {
                let mut enc = match H264VideoEncoder::new() {
                    Ok(e) => e,
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                        return;
                    }
                };
                match source {
                    ClipVideoSource::Camera(device_index) => {
                        let idx = device_index.unwrap_or(0) as u32;
                        let req = RequestedFormat::new::<RgbFormat>(
                            RequestedFormatType::AbsoluteHighestFrameRate,
                        );
                        let mut cam = match Camera::new(CameraIndex::Index(idx), req)
                            .and_then(|mut c| {
                                c.open_stream()?;
                                Ok(c)
                            })
                            .map_err(|e| HuddleError::Video(e.to_string()))
                        {
                            Ok(c) => {
                                let _ = ready_tx.send(Ok(()));
                                c
                            }
                            Err(err) => {
                                let _ = ready_tx.send(Err(err));
                                return;
                            }
                        };
                        while run_flag.load(Ordering::Relaxed) {
                            let tick = Instant::now();
                            if let Ok(frame) = cam.frame() {
                                if let Ok(img) = frame.decode_image::<RgbFormat>() {
                                    let (rgb, w, h) = resize_rgb_even(
                                        img.as_raw(),
                                        img.width(),
                                        img.height(),
                                        CAMERA_MAX_W,
                                        CAMERA_MAX_H,
                                    );
                                    if w >= 2 && h >= 2 {
                                        record_encoded_frame(
                                            &mut enc,
                                            &rgb,
                                            w,
                                            h,
                                            &dims_cb,
                                            &preview_cb,
                                            &frames_cb,
                                        );
                                    }
                                }
                            }
                            if frames_cb.lock().is_ok_and(|f| f.len() >= MAX_VIDEO_FRAMES) {
                                break;
                            }
                            let sleep = Duration::from_millis(1000 / CLIP_FPS as u64)
                                .saturating_sub(tick.elapsed());
                            if !sleep.is_zero() {
                                std::thread::sleep(sleep);
                            }
                        }
                        let _ = cam.stop_stream();
                    }
                    ClipVideoSource::Screen(source_id) => {
                        let _ = ready_tx.send(Ok(()));
                        while run_flag.load(Ordering::Relaxed) {
                            let tick = Instant::now();
                            if let Some(rgba) = crate::screen::capture_once(&source_id) {
                                let (rgb_full, _) = rgba_to_rgb_and_bgra(rgba.as_raw());
                                let (rgb, w, h) = resize_rgb_even(
                                    &rgb_full,
                                    rgba.width(),
                                    rgba.height(),
                                    SCREEN_MAX_W,
                                    SCREEN_MAX_H,
                                );
                                if w >= 2 && h >= 2 {
                                    record_encoded_frame(
                                        &mut enc,
                                        &rgb,
                                        w,
                                        h,
                                        &dims_cb,
                                        &preview_cb,
                                        &frames_cb,
                                    );
                                }
                            }
                            if frames_cb.lock().is_ok_and(|f| f.len() >= MAX_VIDEO_FRAMES) {
                                break;
                            }
                            let sleep = Duration::from_millis(1000 / CLIP_FPS as u64)
                                .saturating_sub(tick.elapsed());
                            if !sleep.is_zero() {
                                std::thread::sleep(sleep);
                            }
                        }
                    }
                }
            })
            .map_err(|e| HuddleError::Video(e.to_string()))?;

        ready_rx
            .recv()
            .map_err(|e| HuddleError::Video(e.to_string()))??;

        Ok(Self {
            running,
            latest_preview,
            frames,
            audio_pcm,
            dimensions,
            worker: Some(worker),
            audio_worker,
            started_at: Instant::now(),
        })
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }

    pub fn latest_preview(&self) -> Option<VideoFrame> {
        self.latest_preview.lock().ok().and_then(|g| g.clone())
    }

    pub fn cancel(mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
        if let Some(h) = self.audio_worker.take() {
            let _ = h.join();
        }
    }

    /// Stop capturing and mux H.264 video (+ synchronized AAC audio when available) into `.mp4`.
    pub fn stop(mut self) -> Result<RecordedClip> {
        let elapsed_ms = self.elapsed_ms();
        self.running.store(false, Ordering::Relaxed);
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
        if let Some(h) = self.audio_worker.take() {
            let _ = h.join();
        }
        let preview_frame = self.latest_preview();
        let (w, h) = self.dimensions.lock().map(|g| *g).unwrap_or((0, 0));
        let frames = self
            .frames
            .lock()
            .map(|g| g.iter().map(|f| f.annex_b.clone()).collect::<Vec<_>>())
            .map_err(|_| HuddleError::Video("video clip buffer poisoned".into()))?;
        if frames.is_empty() || w < 2 || h < 2 {
            return Err(HuddleError::Video("no video frames captured".into()));
        }
        let pcm = self.audio_pcm.lock().map(|g| g.clone()).unwrap_or_default();
        let aac_track = (!pcm.is_empty())
            .then(|| {
                let wav = encode_wav_pcm16(&pcm, SAMPLE_RATE, 1);
                transcode_wav_to_aac_adts(&wav).and_then(|adts| parse_adts_frames(&adts))
            })
            .flatten();
        let fps = ((frames.len() as u64 * 1000) / elapsed_ms.max(250)).clamp(4, 30) as u32;
        let mp4 = mux_h264_aac_mp4(w, h, fps, &frames, aac_track.as_ref())?;
        let slug = timestamp_slug();
        Ok(RecordedClip {
            filename: format!("video-clip-{slug}.mp4"),
            mime: "video/mp4".to_owned(),
            bytes: mp4,
            duration_ms: elapsed_ms,
            waveform: if pcm.is_empty() {
                Vec::new()
            } else {
                downsample_waveform(&pcm, 32)
            },
            preview_frame,
        })
    }
}

impl Drop for VideoClipRecorder {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
        if let Some(h) = self.audio_worker.take() {
            let _ = h.join();
        }
    }
}

fn record_encoded_frame(
    enc: &mut H264VideoEncoder,
    rgb: &[u8],
    w: u32,
    h: u32,
    dims_cb: &Arc<Mutex<(u32, u32)>>,
    preview_cb: &Arc<Mutex<Option<VideoFrame>>>,
    frames_cb: &Arc<Mutex<Vec<EncodedH264Frame>>>,
) {
    // Keep dimensions consistent across a clip (resize if the source window resized mid-clip).
    let (target_w, target_h) = {
        let Ok(mut d) = dims_cb.lock() else { return };
        if *d == (0, 0) {
            *d = (w, h);
        }
        *d
    };
    let (enc_rgb, ew, eh) = if (w, h) == (target_w, target_h) {
        (rgb.to_vec(), w, h)
    } else {
        resize_rgb_even(rgb, w, h, target_w, target_h)
    };
    if ew != target_w || eh != target_h {
        return;
    }
    let bgra = rgb_to_bgra(&enc_rgb);
    if let Ok(mut p) = preview_cb.lock() {
        *p = Some(VideoFrame {
            width: ew,
            height: eh,
            bgra: Arc::new(bgra),
        });
    }
    if let Ok(annex_b) = enc.encode_rgb(&enc_rgb, ew as usize, eh as usize) {
        if !annex_b.is_empty() {
            if let Ok(mut list) = frames_cb.lock() {
                list.push(EncodedH264Frame { annex_b });
            }
        }
    }
}

fn resample_mono(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if input.is_empty() || from_rate == 0 || to_rate == 0 || from_rate == to_rate {
        return input.to_vec();
    }
    let out_len = ((input.len() as u64 * to_rate as u64) / from_rate as u64).max(1) as usize;
    let ratio = from_rate as f64 / to_rate as f64;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as f64 * ratio;
        let idx = (pos as usize).min(input.len() - 1);
        let next = (idx + 1).min(input.len() - 1);
        let frac = (pos - idx as f64) as f32;
        out.push(input[idx] * (1.0 - frac) + input[next] * frac);
    }
    out
}

/// Encode signed 16-bit PCM samples into a standard RIFF/WAVE file.
pub fn encode_wav_pcm16(samples: &[i16], sample_rate: u32, channels: u16) -> Vec<u8> {
    let data_bytes = (samples.len() * 2) as u32;
    let byte_rate = sample_rate * channels as u32 * 2;
    let block_align = channels * 2;
    let mut out = Vec::with_capacity(44 + data_bytes as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_bytes).to_le_bytes());
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

/// On macOS, transcode PCM WAV bytes into a compact AAC `.m4a` via `/usr/bin/afconvert`.
#[cfg(target_os = "macos")]
fn transcode_wav_to_m4a(wav: &[u8]) -> Option<Vec<u8>> {
    let dir = std::env::temp_dir();
    let stamp = timestamp_slug();
    let pid = std::process::id();
    let wav_path = dir.join(format!("slackless-clip-{pid}-{stamp}.wav"));
    let m4a_path = dir.join(format!("slackless-clip-{pid}-{stamp}.m4a"));
    std::fs::write(&wav_path, wav).ok()?;
    let status = std::process::Command::new("/usr/bin/afconvert")
        .args(["-f", "m4af", "-d", "aac", "-b", "96000"])
        .arg(&wav_path)
        .arg(&m4a_path)
        .status()
        .ok()?;
    let _ = std::fs::remove_file(&wav_path);
    if !status.success() {
        let _ = std::fs::remove_file(&m4a_path);
        return None;
    }
    let bytes = std::fs::read(&m4a_path).ok();
    let _ = std::fs::remove_file(&m4a_path);
    bytes.filter(|b| !b.is_empty())
}

#[cfg(not(target_os = "macos"))]
fn transcode_wav_to_m4a(_wav: &[u8]) -> Option<Vec<u8>> {
    None
}

/// On macOS, transcode PCM WAV bytes into ADTS AAC frames via `/usr/bin/afconvert` for MP4 muxing.
#[cfg(target_os = "macos")]
fn transcode_wav_to_aac_adts(wav: &[u8]) -> Option<Vec<u8>> {
    let dir = std::env::temp_dir();
    let stamp = timestamp_slug();
    let pid = std::process::id();
    let wav_path = dir.join(format!("slackless-vidclip-{pid}-{stamp}.wav"));
    let aac_path = dir.join(format!("slackless-vidclip-{pid}-{stamp}.aac"));
    std::fs::write(&wav_path, wav).ok()?;
    let status = std::process::Command::new("/usr/bin/afconvert")
        .args(["-f", "adts", "-d", "aac", "-b", "96000"])
        .arg(&wav_path)
        .arg(&aac_path)
        .status()
        .ok()?;
    let _ = std::fs::remove_file(&wav_path);
    if !status.success() {
        let _ = std::fs::remove_file(&aac_path);
        return None;
    }
    let bytes = std::fs::read(&aac_path).ok();
    let _ = std::fs::remove_file(&aac_path);
    bytes.filter(|b| !b.is_empty())
}

#[cfg(not(target_os = "macos"))]
fn transcode_wav_to_aac_adts(_wav: &[u8]) -> Option<Vec<u8>> {
    None
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
            // AudioSpecificConfig: 5 bits objectType | 4 bits freqIndex | 4 bits chanConfig | 3 bits 0
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

// ---------------------------------------------------------------------------
// Pure-Rust ISO-BMFF H.264 `.mp4` muxer (`ftyp` + `mdat` + `moov`/`avcC`)
// ---------------------------------------------------------------------------

fn split_annex_b_nals(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 {
            if i + 4 <= data.len() && data[i + 2] == 0 && data[i + 3] == 1 {
                starts.push((i, 4));
                i += 4;
                continue;
            }
            if data[i + 2] == 1 {
                starts.push((i, 3));
                i += 3;
                continue;
            }
        }
        i += 1;
    }
    let mut nals = Vec::with_capacity(starts.len());
    for idx in 0..starts.len() {
        let (pos, sc_len) = starts[idx];
        let end = starts.get(idx + 1).map_or(data.len(), |&(next, _)| next);
        let nal = &data[pos + sc_len..end];
        if !nal.is_empty() {
            nals.push(nal);
        }
    }
    nals
}

fn mp4_box(fourcc: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let size = (8 + payload.len()) as u32;
    let mut out = Vec::with_capacity(size as usize);
    out.extend_from_slice(&size.to_be_bytes());
    out.extend_from_slice(fourcc);
    out.extend_from_slice(payload);
    out
}

fn mp4_full_box(fourcc: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(4 + payload.len());
    let vf = ((version as u32) << 24) | (flags & 0x00FF_FFFF);
    body.extend_from_slice(&vf.to_be_bytes());
    body.extend_from_slice(payload);
    mp4_box(fourcc, &body)
}

/// Mux Annex-B H.264 access units into a self-contained ISO-BMFF `.mp4` container.
pub fn mux_h264_mp4(width: u32, height: u32, fps: u32, frames: &[Vec<u8>]) -> Result<Vec<u8>> {
    mux_h264_aac_mp4(width, height, fps, frames, None)
}

/// Mux Annex-B H.264 video frames + optional raw AAC-LC audio track into an ISO-BMFF `.mp4`.
pub fn mux_h264_aac_mp4(
    width: u32,
    height: u32,
    fps: u32,
    frames: &[Vec<u8>],
    audio: Option<&AacAudioTrack>,
) -> Result<Vec<u8>> {
    let fps = fps.max(1);
    let timescale: u32 = 90_000;
    let sample_delta: u32 = timescale / fps;

    let mut sps: Option<Vec<u8>> = None;
    let mut pps: Option<Vec<u8>> = None;
    let mut mdat_payload = Vec::new();
    let mut sample_sizes: Vec<u32> = Vec::with_capacity(frames.len());
    let mut sync_samples: Vec<u32> = Vec::new();

    for frame in frames {
        let nals = split_annex_b_nals(frame);
        let sample_start = mdat_payload.len();
        let mut is_idr = false;
        for nal in nals {
            let nal_type = nal[0] & 0x1F;
            match nal_type {
                7 => {
                    if sps.is_none() {
                        sps = Some(nal.to_vec());
                    }
                }
                8 => {
                    if pps.is_none() {
                        pps = Some(nal.to_vec());
                    }
                }
                9 => {} // Access unit delimiter; skip in MP4 sample payload
                _ => {
                    if nal_type == 5 {
                        is_idr = true;
                    }
                    mdat_payload.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                    mdat_payload.extend_from_slice(nal);
                }
            }
        }
        let sample_len = (mdat_payload.len() - sample_start) as u32;
        if sample_len > 0 {
            sample_sizes.push(sample_len);
            if is_idr {
                sync_samples.push(sample_sizes.len() as u32);
            }
        }
    }

    let sps = sps.ok_or_else(|| HuddleError::Codec("H.264 stream missing SPS NAL unit".into()))?;
    let pps = pps.ok_or_else(|| HuddleError::Codec("H.264 stream missing PPS NAL unit".into()))?;
    if sample_sizes.is_empty() {
        return Err(HuddleError::Codec(
            "H.264 stream contains no video samples".into(),
        ));
    }
    if sync_samples.is_empty() {
        sync_samples.push(1);
    }

    // ftyp box
    let mut ftyp_body = Vec::new();
    ftyp_body.extend_from_slice(b"isom");
    ftyp_body.extend_from_slice(&0x0000_0200u32.to_be_bytes());
    ftyp_body.extend_from_slice(b"isomiso2avc1mp41");
    let ftyp = mp4_box(b"ftyp", &ftyp_body);

    let video_chunk_offset = (ftyp.len() + 8) as u32;
    let audio_chunk_offset = (ftyp.len() + 8 + mdat_payload.len()) as u32;
    let mut audio_sizes: Vec<u32> = Vec::new();
    if let Some(a) = audio {
        audio_sizes.reserve(a.frames.len());
        for frame in &a.frames {
            mdat_payload.extend_from_slice(frame);
            audio_sizes.push(frame.len() as u32);
        }
    }
    let mdat = mp4_box(b"mdat", &mdat_payload);
    let mdat_data_offset = video_chunk_offset;

    let total_duration = sample_sizes.len() as u32 * sample_delta;

    // avcC box (ISO/IEC 14496-15)
    let profile_idc = *sps.get(1).unwrap_or(&66);
    let profile_compat = *sps.get(2).unwrap_or(&0);
    let level_idc = *sps.get(3).unwrap_or(&31);
    let mut avcc_body = vec![
        1, // configurationVersion
        profile_idc,
        profile_compat,
        level_idc,
        0xFF, // 4-byte NALU length prefix (lengthSizeMinusOne = 3)
        0xE1, // numOfSequenceParameterSets = 1
    ];
    avcc_body.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    avcc_body.extend_from_slice(&sps);
    avcc_body.push(1); // numOfPictureParameterSets = 1
    avcc_body.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    avcc_body.extend_from_slice(&pps);
    let avcc = mp4_box(b"avcC", &avcc_body);

    // avc1 sample entry
    let mut avc1_body = Vec::new();
    avc1_body.extend_from_slice(&[0u8; 6]); // reserved
    avc1_body.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    avc1_body.extend_from_slice(&[0u8; 16]); // pre_defined + reserved
    avc1_body.extend_from_slice(&(width as u16).to_be_bytes());
    avc1_body.extend_from_slice(&(height as u16).to_be_bytes());
    avc1_body.extend_from_slice(&0x0048_0000u32.to_be_bytes()); // horizresolution 72 dpi
    avc1_body.extend_from_slice(&0x0048_0000u32.to_be_bytes()); // vertresolution 72 dpi
    avc1_body.extend_from_slice(&0u32.to_be_bytes()); // reserved
    avc1_body.extend_from_slice(&1u16.to_be_bytes()); // frame_count
    avc1_body.extend_from_slice(&[0u8; 32]); // compressorname
    avc1_body.extend_from_slice(&0x0018u16.to_be_bytes()); // depth = 24
    avc1_body.extend_from_slice(&(-1i16).to_be_bytes()); // pre_defined = -1
    avc1_body.extend_from_slice(&avcc);
    let avc1 = mp4_box(b"avc1", &avc1_body);

    let mut stsd_body = Vec::new();
    stsd_body.extend_from_slice(&1u32.to_be_bytes()); // entry_count
    stsd_body.extend_from_slice(&avc1);
    let stsd = mp4_full_box(b"stsd", 0, 0, &stsd_body);

    let mut stts_body = Vec::new();
    stts_body.extend_from_slice(&1u32.to_be_bytes());
    stts_body.extend_from_slice(&(sample_sizes.len() as u32).to_be_bytes());
    stts_body.extend_from_slice(&sample_delta.to_be_bytes());
    let stts = mp4_full_box(b"stts", 0, 0, &stts_body);

    let mut stss_body = Vec::new();
    stss_body.extend_from_slice(&(sync_samples.len() as u32).to_be_bytes());
    for &idx in &sync_samples {
        stss_body.extend_from_slice(&idx.to_be_bytes());
    }
    let stss = mp4_full_box(b"stss", 0, 0, &stss_body);

    let mut stsc_body = Vec::new();
    stsc_body.extend_from_slice(&1u32.to_be_bytes()); // entry_count = 1 chunk holds all samples
    stsc_body.extend_from_slice(&1u32.to_be_bytes()); // first_chunk
    stsc_body.extend_from_slice(&(sample_sizes.len() as u32).to_be_bytes()); // samples_per_chunk
    stsc_body.extend_from_slice(&1u32.to_be_bytes()); // sample_description_index
    let stsc = mp4_full_box(b"stsc", 0, 0, &stsc_body);

    let mut stsz_body = Vec::new();
    stsz_body.extend_from_slice(&0u32.to_be_bytes()); // variable sample sizes
    stsz_body.extend_from_slice(&(sample_sizes.len() as u32).to_be_bytes());
    for &sz in &sample_sizes {
        stsz_body.extend_from_slice(&sz.to_be_bytes());
    }
    let stsz = mp4_full_box(b"stsz", 0, 0, &stsz_body);

    let mut stco_body = Vec::new();
    stco_body.extend_from_slice(&1u32.to_be_bytes());
    stco_body.extend_from_slice(&mdat_data_offset.to_be_bytes());
    let stco = mp4_full_box(b"stco", 0, 0, &stco_body);

    let mut stbl_body = Vec::new();
    for b in [&stsd, &stts, &stss, &stsc, &stsz, &stco] {
        stbl_body.extend_from_slice(b);
    }
    let stbl = mp4_box(b"stbl", &stbl_body);

    let vmhd = mp4_full_box(b"vmhd", 0, 1, &[0u8; 8]);
    let url_box = mp4_full_box(b"url ", 0, 1, &[]);
    let mut dref_body = Vec::new();
    dref_body.extend_from_slice(&1u32.to_be_bytes());
    dref_body.extend_from_slice(&url_box);
    let dref = mp4_full_box(b"dref", 0, 0, &dref_body);
    let dinf = mp4_box(b"dinf", &dref);

    let mut minf_body = Vec::new();
    minf_body.extend_from_slice(&vmhd);
    minf_body.extend_from_slice(&dinf);
    minf_body.extend_from_slice(&stbl);
    let minf = mp4_box(b"minf", &minf_body);

    let mut mdhd_body = Vec::new();
    mdhd_body.extend_from_slice(&0u32.to_be_bytes()); // creation_time
    mdhd_body.extend_from_slice(&0u32.to_be_bytes()); // modification_time
    mdhd_body.extend_from_slice(&timescale.to_be_bytes());
    mdhd_body.extend_from_slice(&total_duration.to_be_bytes());
    mdhd_body.extend_from_slice(&0x55C4u16.to_be_bytes()); // und language
    mdhd_body.extend_from_slice(&0u16.to_be_bytes());
    let mdhd = mp4_full_box(b"mdhd", 0, 0, &mdhd_body);

    let mut hdlr_body = Vec::new();
    hdlr_body.extend_from_slice(&0u32.to_be_bytes()); // pre_defined
    hdlr_body.extend_from_slice(b"vide");
    hdlr_body.extend_from_slice(&[0u8; 12]); // reserved
    hdlr_body.extend_from_slice(b"VideoHandler\0");
    let hdlr = mp4_full_box(b"hdlr", 0, 0, &hdlr_body);

    let mut mdia_body = Vec::new();
    mdia_body.extend_from_slice(&mdhd);
    mdia_body.extend_from_slice(&hdlr);
    mdia_body.extend_from_slice(&minf);
    let mdia = mp4_box(b"mdia", &mdia_body);

    let unity_matrix: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];
    let mut tkhd_body = Vec::new();
    tkhd_body.extend_from_slice(&0u32.to_be_bytes()); // creation_time
    tkhd_body.extend_from_slice(&0u32.to_be_bytes()); // modification_time
    tkhd_body.extend_from_slice(&1u32.to_be_bytes()); // track_id
    tkhd_body.extend_from_slice(&0u32.to_be_bytes()); // reserved
    tkhd_body.extend_from_slice(&total_duration.to_be_bytes());
    tkhd_body.extend_from_slice(&[0u8; 8]); // reserved
    tkhd_body.extend_from_slice(&0u16.to_be_bytes()); // layer
    tkhd_body.extend_from_slice(&0u16.to_be_bytes()); // alternate_group
    tkhd_body.extend_from_slice(&0u16.to_be_bytes()); // volume (video = 0)
    tkhd_body.extend_from_slice(&0u16.to_be_bytes()); // reserved
    for m in unity_matrix {
        tkhd_body.extend_from_slice(&m.to_be_bytes());
    }
    tkhd_body.extend_from_slice(&(width << 16).to_be_bytes());
    tkhd_body.extend_from_slice(&(height << 16).to_be_bytes());
    let tkhd = mp4_full_box(b"tkhd", 0, 0x0000_0003, &tkhd_body); // enabled + in_movie

    let mut trak_body = Vec::new();
    trak_body.extend_from_slice(&tkhd);
    trak_body.extend_from_slice(&mdia);
    let trak = mp4_box(b"trak", &trak_body);

    let next_track_id: u32 = if audio.is_some() { 3 } else { 2 };
    let mut mvhd_body = Vec::new();
    mvhd_body.extend_from_slice(&0u32.to_be_bytes());
    mvhd_body.extend_from_slice(&0u32.to_be_bytes());
    mvhd_body.extend_from_slice(&timescale.to_be_bytes());
    mvhd_body.extend_from_slice(&total_duration.to_be_bytes());
    mvhd_body.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // rate 1.0
    mvhd_body.extend_from_slice(&0x0100u16.to_be_bytes()); // volume 1.0
    mvhd_body.extend_from_slice(&[0u8; 10]); // reserved
    for m in unity_matrix {
        mvhd_body.extend_from_slice(&m.to_be_bytes());
    }
    mvhd_body.extend_from_slice(&[0u8; 24]); // pre_defined
    mvhd_body.extend_from_slice(&next_track_id.to_be_bytes());
    let mvhd = mp4_full_box(b"mvhd", 0, 0, &mvhd_body);

    let mut moov_body = Vec::new();
    moov_body.extend_from_slice(&mvhd);
    moov_body.extend_from_slice(&trak);
    if let Some(a) = audio {
        if !audio_sizes.is_empty() {
            let audio_trak = build_aac_mp4_trak(
                a,
                &audio_sizes,
                audio_chunk_offset,
                total_duration,
                unity_matrix,
            );
            moov_body.extend_from_slice(&audio_trak);
        }
    }
    let moov = mp4_box(b"moov", &moov_body);

    let mut out = Vec::with_capacity(ftyp.len() + mdat.len() + moov.len());
    out.extend_from_slice(&ftyp);
    out.extend_from_slice(&mdat);
    out.extend_from_slice(&moov);
    Ok(out)
}

fn build_aac_mp4_trak(
    audio: &AacAudioTrack,
    sample_sizes: &[u32],
    chunk_offset: u32,
    movie_duration: u32,
    unity_matrix: [u32; 9],
) -> Vec<u8> {
    let audio_timescale = audio.sample_rate.max(8_000);
    let sample_delta = 1024u32; // AAC-LC frame length
    let audio_duration = sample_sizes.len() as u32 * sample_delta;

    // MPEG-4 ES_Descriptor (`esds`) carrying DecoderSpecificInfo = AudioSpecificConfig.
    let asc = audio.audio_specific_config;
    let dec_spec = vec![0x05, 0x02, asc[0], asc[1]];
    let mut dec_cfg = vec![
        0x04,
        (13 + dec_spec.len()) as u8,
        0x40, // objectTypeIndication: MPEG-4 Audio
        0x15, // streamType=Audio (5<<2 | 1)
        0x00,
        0x00,
        0x00, // bufferSizeDB
    ];
    dec_cfg.extend_from_slice(&128_000u32.to_be_bytes()); // maxBitrate
    dec_cfg.extend_from_slice(&96_000u32.to_be_bytes()); // avgBitrate
    dec_cfg.extend_from_slice(&dec_spec);
    let sl_cfg = [0x06u8, 0x01, 0x02];
    let mut es_desc = vec![
        0x03,
        (3 + dec_cfg.len() + sl_cfg.len()) as u8,
        0x00,
        0x02, // ES_ID = 2
        0x00, // flags
    ];
    es_desc.extend_from_slice(&dec_cfg);
    es_desc.extend_from_slice(&sl_cfg);
    let esds = mp4_full_box(b"esds", 0, 0, &es_desc);

    let mut mp4a_body = Vec::new();
    mp4a_body.extend_from_slice(&[0u8; 6]); // reserved
    mp4a_body.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    mp4a_body.extend_from_slice(&[0u8; 8]); // reserved
    mp4a_body.extend_from_slice(&audio.channels.to_be_bytes());
    mp4a_body.extend_from_slice(&16u16.to_be_bytes()); // sampleSize
    mp4a_body.extend_from_slice(&0u32.to_be_bytes()); // pre_defined + reserved
    mp4a_body.extend_from_slice(&(audio_timescale << 16).to_be_bytes()); // 16.16 fixed point
    mp4a_body.extend_from_slice(&esds);
    let mp4a = mp4_box(b"mp4a", &mp4a_body);

    let mut stsd_body = Vec::new();
    stsd_body.extend_from_slice(&1u32.to_be_bytes());
    stsd_body.extend_from_slice(&mp4a);
    let stsd = mp4_full_box(b"stsd", 0, 0, &stsd_body);

    let mut stts_body = Vec::new();
    stts_body.extend_from_slice(&1u32.to_be_bytes());
    stts_body.extend_from_slice(&(sample_sizes.len() as u32).to_be_bytes());
    stts_body.extend_from_slice(&sample_delta.to_be_bytes());
    let stts = mp4_full_box(b"stts", 0, 0, &stts_body);

    let mut stsc_body = Vec::new();
    stsc_body.extend_from_slice(&1u32.to_be_bytes());
    stsc_body.extend_from_slice(&1u32.to_be_bytes());
    stsc_body.extend_from_slice(&(sample_sizes.len() as u32).to_be_bytes());
    stsc_body.extend_from_slice(&1u32.to_be_bytes());
    let stsc = mp4_full_box(b"stsc", 0, 0, &stsc_body);

    let mut stsz_body = Vec::new();
    stsz_body.extend_from_slice(&0u32.to_be_bytes());
    stsz_body.extend_from_slice(&(sample_sizes.len() as u32).to_be_bytes());
    for &sz in sample_sizes {
        stsz_body.extend_from_slice(&sz.to_be_bytes());
    }
    let stsz = mp4_full_box(b"stsz", 0, 0, &stsz_body);

    let mut stco_body = Vec::new();
    stco_body.extend_from_slice(&1u32.to_be_bytes());
    stco_body.extend_from_slice(&chunk_offset.to_be_bytes());
    let stco = mp4_full_box(b"stco", 0, 0, &stco_body);

    let mut stbl_body = Vec::new();
    for b in [&stsd, &stts, &stsc, &stsz, &stco] {
        stbl_body.extend_from_slice(b);
    }
    let stbl = mp4_box(b"stbl", &stbl_body);

    let smhd = mp4_full_box(b"smhd", 0, 0, &[0u8; 4]);
    let url_box = mp4_full_box(b"url ", 0, 1, &[]);
    let mut dref_body = Vec::new();
    dref_body.extend_from_slice(&1u32.to_be_bytes());
    dref_body.extend_from_slice(&url_box);
    let dref = mp4_full_box(b"dref", 0, 0, &dref_body);
    let dinf = mp4_box(b"dinf", &dref);

    let mut minf_body = Vec::new();
    minf_body.extend_from_slice(&smhd);
    minf_body.extend_from_slice(&dinf);
    minf_body.extend_from_slice(&stbl);
    let minf = mp4_box(b"minf", &minf_body);

    let mut mdhd_body = Vec::new();
    mdhd_body.extend_from_slice(&0u32.to_be_bytes());
    mdhd_body.extend_from_slice(&0u32.to_be_bytes());
    mdhd_body.extend_from_slice(&audio_timescale.to_be_bytes());
    mdhd_body.extend_from_slice(&audio_duration.to_be_bytes());
    mdhd_body.extend_from_slice(&0x55C4u16.to_be_bytes());
    mdhd_body.extend_from_slice(&0u16.to_be_bytes());
    let mdhd = mp4_full_box(b"mdhd", 0, 0, &mdhd_body);

    let mut hdlr_body = Vec::new();
    hdlr_body.extend_from_slice(&0u32.to_be_bytes());
    hdlr_body.extend_from_slice(b"soun");
    hdlr_body.extend_from_slice(&[0u8; 12]);
    hdlr_body.extend_from_slice(b"SoundHandler\0");
    let hdlr = mp4_full_box(b"hdlr", 0, 0, &hdlr_body);

    let mut mdia_body = Vec::new();
    mdia_body.extend_from_slice(&mdhd);
    mdia_body.extend_from_slice(&hdlr);
    mdia_body.extend_from_slice(&minf);
    let mdia = mp4_box(b"mdia", &mdia_body);

    let mut tkhd_body = Vec::new();
    tkhd_body.extend_from_slice(&0u32.to_be_bytes());
    tkhd_body.extend_from_slice(&0u32.to_be_bytes());
    tkhd_body.extend_from_slice(&2u32.to_be_bytes()); // track_id = 2
    tkhd_body.extend_from_slice(&0u32.to_be_bytes());
    tkhd_body.extend_from_slice(&movie_duration.to_be_bytes());
    tkhd_body.extend_from_slice(&[0u8; 8]);
    tkhd_body.extend_from_slice(&0u16.to_be_bytes());
    tkhd_body.extend_from_slice(&1u16.to_be_bytes()); // alternate_group
    tkhd_body.extend_from_slice(&0x0100u16.to_be_bytes()); // volume = 1.0
    tkhd_body.extend_from_slice(&0u16.to_be_bytes());
    for m in unity_matrix {
        tkhd_body.extend_from_slice(&m.to_be_bytes());
    }
    tkhd_body.extend_from_slice(&0u32.to_be_bytes()); // width = 0
    tkhd_body.extend_from_slice(&0u32.to_be_bytes()); // height = 0
    let tkhd = mp4_full_box(b"tkhd", 0, 0x0000_0003, &tkhd_body);

    let mut trak_body = Vec::new();
    trak_body.extend_from_slice(&tkhd);
    trak_body.extend_from_slice(&mdia);
    mp4_box(b"trak", &trak_body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_pcm16_has_valid_riff_header_and_length() {
        let samples = vec![0i16, 1000, -1000, 32767, -32768];
        let wav = encode_wav_pcm16(&samples, 48_000, 1);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(wav.len(), 44 + samples.len() * 2);
    }

    #[test]
    fn waveform_downsamples_to_requested_bars() {
        let mut samples = Vec::new();
        for i in 0..4800 {
            let amp = if i < 2400 { 4000 } else { 20000 };
            samples.push(if i % 2 == 0 { amp } else { -amp });
        }
        let bars = downsample_waveform(&samples, 16);
        assert_eq!(bars.len(), 16);
        assert!(bars.iter().all(|&b| (0.08..=1.0).contains(&b)));
        assert!(bars[12] > bars[2]);
    }

    #[test]
    fn mux_h264_mp4_produces_valid_iso_bmff_with_avcc() {
        let mut enc = H264VideoEncoder::new().expect("openh264 encoder");
        let (w, h) = (64usize, 64usize);
        let mut frames = Vec::new();
        for i in 0..4u8 {
            let mut rgb = vec![0u8; w * h * 3];
            for px in rgb.as_chunks_mut::<3>().0 {
                px[0] = i.wrapping_mul(50);
                px[1] = 120;
                px[2] = 200;
            }
            frames.push(enc.encode_rgb(&rgb, w, h).expect("encode"));
        }
        let mp4 = mux_h264_mp4(w as u32, h as u32, 12, &frames).expect("mux mp4");
        assert_eq!(&mp4[4..8], b"ftyp");
        assert!(mp4.windows(4).any(|w| w == b"mdat"));
        assert!(mp4.windows(4).any(|w| w == b"moov"));
        assert!(mp4.windows(4).any(|w| w == b"avcC"));
        assert!(mp4.windows(4).any(|w| w == b"stsz"));
        assert!(mp4.windows(4).any(|w| w == b"stco"));

        let audio = AacAudioTrack {
            sample_rate: 48_000,
            channels: 1,
            audio_specific_config: [0x11, 0x88],
            frames: vec![
                vec![0x21, 0x00, 0x49, 0x90, 0x02, 0x19],
                vec![0x21, 0x00, 0x49, 0x90],
            ],
        };
        let av_mp4 = mux_h264_aac_mp4(w as u32, h as u32, 12, &frames, Some(&audio))
            .expect("mux dual-track mp4");
        assert!(av_mp4.windows(4).any(|w| w == b"avcC"));
        assert!(av_mp4.windows(4).any(|w| w == b"mp4a"));
        assert!(av_mp4.windows(4).any(|w| w == b"esds"));
        assert!(av_mp4.windows(4).any(|w| w == b"soun"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn afconvert_adts_parses_and_muxes_into_dual_track_mp4() {
        // 0.25s 440Hz sine at 48 kHz mono -> real macOS afconvert ADTS -> parse_adts_frames.
        let pcm: Vec<i16> = (0..12_000)
            .map(|i| {
                ((i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 12_000.0) as i16
            })
            .collect();
        let wav = encode_wav_pcm16(&pcm, 48_000, 1);
        let adts = transcode_wav_to_aac_adts(&wav).expect("afconvert adts");
        let track = parse_adts_frames(&adts).expect("parse adts");
        assert_eq!(track.sample_rate, 48_000);
        assert_eq!(track.channels, 1);
        assert!(track.frames.len() >= 10);
    }
}
