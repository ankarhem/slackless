//! Video clip capture (routed through [`crate::backend::MediaBackend`] for cameras and
//! autorelease-pooled `xcap` for screen capture) with synchronized microphone capture.

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::audio::SAMPLE_RATE;
use crate::backend::{CameraPreviewConfig, CameraPreviewHandle, platform_backend};
use crate::screen::{SCREEN_MAX_H, SCREEN_MAX_W, with_autorelease_pool};
use crate::video::{
    CAMERA_MAX_H, CAMERA_MAX_W, H264VideoEncoder, VideoFrame, bgra_to_rgb,
    downscale_rgba_to_rgb_and_bgra_even, resize_rgb_even, rgb_to_bgra,
};
use crate::{HuddleError, Result};

use super::audio::{
    AudioClipRecorder, downsample_waveform, encode_wav_pcm16, parse_adts_frames,
    transcode_wav_to_aac_adts,
};
use super::mp4::mux_h264_aac_mp4;
use super::{CLIP_FPS, ClipVideoSource, MAX_VIDEO_FRAMES, RecordedClip, timestamp_slug};

/// Live camera or screen video clip recorder (`F-CORE-128`) with synchronized microphone audio.
pub struct VideoClipRecorder {
    running: Arc<AtomicBool>,
    latest_preview: Arc<Mutex<Option<VideoFrame>>>,
    thumbnails: Arc<Mutex<Vec<VideoFrame>>>,
    pub(super) frames: Arc<Mutex<Vec<EncodedH264Frame>>>,
    audio_pcm: Arc<Mutex<Vec<i16>>>,
    dimensions: Arc<Mutex<(u32, u32)>>,
    worker: Option<std::thread::JoinHandle<()>>,
    audio_worker: Option<std::thread::JoinHandle<()>>,
    started_at: Instant,
}

pub(super) struct EncodedH264Frame {
    annex_b: Vec<u8>,
}

/// Raw RGB888 frame delivered by a [`ClipFrameSource`] into [`VideoClipRecorder`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CapturedRgbFrame {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// Optional already-built BGRA preview paired with an RGB frame to eliminate redundant BGRA<->RGB
/// conversion passes during clip recording.
pub(super) struct CapturedClipFrame {
    pub rgb: CapturedRgbFrame,
    pub preview: Option<VideoFrame>,
}

/// Injectable frame capture seam for [`VideoClipRecorder`]: implemented by the platform
/// [`crate::backend::MediaBackend`] camera preview, `xcap` screen capture, and test fakes.
pub(super) trait ClipFrameSource {
    fn next_frame(&mut self) -> Option<CapturedRgbFrame>;

    fn next_clip_frame(&mut self) -> Option<CapturedClipFrame> {
        self.next_frame()
            .map(|rgb| CapturedClipFrame { rgb, preview: None })
    }

    fn stop(&mut self) {}
}

/// Convert a [`VideoFrame`] (zero-copy macOS `MacVideoSurface` or portable BGRA) into a tightly
/// packed [`CapturedRgbFrame`] fit within `(max_w, max_h)`.
#[cfg(test)]
pub(crate) fn video_frame_to_clip_rgb(
    frame: &VideoFrame,
    max_w: u32,
    max_h: u32,
) -> Option<CapturedRgbFrame> {
    video_frame_to_clip_pair(frame, max_w, max_h).map(|c| c.rgb)
}

fn video_frame_to_clip_pair(
    frame: &VideoFrame,
    max_w: u32,
    max_h: u32,
) -> Option<CapturedClipFrame> {
    let (w, h, bgra) = frame.to_bgra_fit(max_w, max_h)?;
    if w < 2 || h < 2 || bgra.len() < (w as usize) * (h as usize) * 4 {
        return None;
    }
    let rgb = bgra_to_rgb(&bgra);
    let preview = VideoFrame::from_bgra(w, h, bgra);
    Some(CapturedClipFrame {
        rgb: CapturedRgbFrame {
            width: w,
            height: h,
            rgb,
        },
        preview: Some(preview),
    })
}

/// Cross-platform camera source routed cleanly through [`crate::backend::MediaBackend`].
struct BackendClipCamera {
    capture: Box<dyn CameraPreviewHandle>,
    latest: Arc<Mutex<Option<VideoFrame>>>,
    max_w: u32,
    max_h: u32,
}

impl BackendClipCamera {
    fn open(device_index: Option<usize>, max_w: u32, max_h: u32) -> Result<Self> {
        let latest: Arc<Mutex<Option<VideoFrame>>> = Arc::new(Mutex::new(None));
        let slot = Arc::clone(&latest);
        let capture = platform_backend().start_camera_preview(
            CameraPreviewConfig {
                device_index,
                max_w,
                max_h,
                max_fps: CLIP_FPS,
                blur_background: false,
            },
            Arc::new(move |frame| {
                if let Ok(mut guard) = slot.lock() {
                    *guard = Some(frame);
                }
            }),
        )?;
        Ok(Self {
            capture,
            latest,
            max_w,
            max_h,
        })
    }
}

impl ClipFrameSource for BackendClipCamera {
    fn next_frame(&mut self) -> Option<CapturedRgbFrame> {
        self.next_clip_frame().map(|c| c.rgb)
    }

    fn next_clip_frame(&mut self) -> Option<CapturedClipFrame> {
        let frame = self.latest.lock().ok().and_then(|mut g| g.take())?;
        video_frame_to_clip_pair(&frame, self.max_w, self.max_h)
    }

    fn stop(&mut self) {
        self.capture.stop();
        if let Ok(mut guard) = self.latest.lock() {
            *guard = None;
        }
    }
}

struct ScreenClipSource {
    source_id: String,
    max_w: u32,
    max_h: u32,
}

impl ClipFrameSource for ScreenClipSource {
    fn next_frame(&mut self) -> Option<CapturedRgbFrame> {
        self.next_clip_frame().map(|c| c.rgb)
    }

    fn next_clip_frame(&mut self) -> Option<CapturedClipFrame> {
        // Drain Objective-C autoreleased objects on plain Rust worker thread.
        let rgba = with_autorelease_pool(|| crate::screen::capture_once(&self.source_id))?;
        let (rgb, bgra, w, h) = downscale_rgba_to_rgb_and_bgra_even(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            self.max_w,
            self.max_h,
        );
        if w < 2 || h < 2 {
            return None;
        }
        Some(CapturedClipFrame {
            rgb: CapturedRgbFrame {
                width: w,
                height: h,
                rgb,
            },
            preview: Some(VideoFrame::from_bgra(w, h, bgra)),
        })
    }
}

struct ClipEncodeState {
    dims: Arc<Mutex<(u32, u32)>>,
    preview: Arc<Mutex<Option<VideoFrame>>>,
    thumbs: Arc<Mutex<Vec<VideoFrame>>>,
    frames: Arc<Mutex<Vec<EncodedH264Frame>>>,
}

impl VideoClipRecorder {
    /// Start capturing camera or screen frames into H.264 plus synchronized microphone audio.
    pub fn start(source: ClipVideoSource) -> Result<Self> {
        let frame_interval = Duration::from_millis(1000 / CLIP_FPS as u64);
        match source {
            ClipVideoSource::Camera(device_index) => Self::start_with_source_factory(
                move || BackendClipCamera::open(device_index, CAMERA_MAX_W, CAMERA_MAX_H),
                CAMERA_MAX_W,
                CAMERA_MAX_H,
                true,
                frame_interval,
            ),
            ClipVideoSource::Screen(source_id) => Self::start_with_source_factory(
                move || {
                    Ok(ScreenClipSource {
                        source_id,
                        max_w: SCREEN_MAX_W,
                        max_h: SCREEN_MAX_H,
                    })
                },
                SCREEN_MAX_W,
                SCREEN_MAX_H,
                true,
                frame_interval,
            ),
        }
    }

    pub(super) fn start_with_source_factory<S, F>(
        open_source: F,
        max_w: u32,
        max_h: u32,
        enable_audio: bool,
        frame_interval: Duration,
    ) -> Result<Self>
    where
        S: ClipFrameSource,
        F: FnOnce() -> Result<S> + Send + 'static,
    {
        let running = Arc::new(AtomicBool::new(true));
        let latest_preview: Arc<Mutex<Option<VideoFrame>>> = Arc::new(Mutex::new(None));
        let thumbnails: Arc<Mutex<Vec<VideoFrame>>> = Arc::new(Mutex::new(Vec::with_capacity(4)));
        let frames: Arc<Mutex<Vec<EncodedH264Frame>>> =
            Arc::new(Mutex::new(Vec::with_capacity(120)));
        let audio_pcm: Arc<Mutex<Vec<i16>>> =
            Arc::new(Mutex::new(Vec::with_capacity((SAMPLE_RATE as usize) * 15)));
        let dimensions: Arc<Mutex<(u32, u32)>> = Arc::new(Mutex::new((0, 0)));

        let run_flag = Arc::clone(&running);
        let encode_state = ClipEncodeState {
            dims: Arc::clone(&dimensions),
            preview: Arc::clone(&latest_preview),
            thumbs: Arc::clone(&thumbnails),
            frames: Arc::clone(&frames),
        };
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<()>>(1);

        let audio_worker = if enable_audio {
            let audio_run = Arc::clone(&running);
            let audio_buf = Arc::clone(&audio_pcm);
            std::thread::Builder::new()
                .name("slackless-video-clip-audio".to_owned())
                .spawn(move || {
                    let Ok(rec) = AudioClipRecorder::start() else {
                        return;
                    };
                    while audio_run.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(40));
                    }
                    if let Ok((pcm, _)) = rec.stop_to_pcm()
                        && let Ok(mut dst) = audio_buf.lock()
                    {
                        *dst = pcm;
                    }
                })
                .ok()
        } else {
            None
        };

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
                let mut source = match open_source() {
                    Ok(src) => {
                        let _ = ready_tx.send(Ok(()));
                        src
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                        return;
                    }
                };
                while run_flag.load(Ordering::Relaxed) {
                    let tick = Instant::now();
                    if let Some(captured) = source.next_clip_frame() {
                        let c = captured.rgb;
                        if c.rgb.len() >= (c.width as usize) * (c.height as usize) * 3 {
                            let already_even_fit = c.width >= 2
                                && c.height >= 2
                                && c.width.is_multiple_of(2)
                                && c.height.is_multiple_of(2)
                                && c.width <= max_w
                                && c.height <= max_h;
                            let (rgb, w, h, preview): (
                                Cow<'_, [u8]>,
                                u32,
                                u32,
                                Option<VideoFrame>,
                            ) = if already_even_fit {
                                (Cow::Borrowed(&c.rgb), c.width, c.height, captured.preview)
                            } else {
                                let (resized, rw, rh) =
                                    resize_rgb_even(&c.rgb, c.width, c.height, max_w, max_h);
                                (Cow::Owned(resized), rw, rh, None)
                            };
                            if w >= 2 && h >= 2 {
                                record_encoded_frame(&mut enc, &rgb, w, h, preview, &encode_state);
                            }
                        }
                    }
                    if encode_state
                        .frames
                        .lock()
                        .is_ok_and(|f| f.len() >= MAX_VIDEO_FRAMES)
                    {
                        break;
                    }
                    let sleep = frame_interval.saturating_sub(tick.elapsed());
                    if !sleep.is_zero() {
                        std::thread::sleep(sleep);
                    }
                }
                source.stop();
            })
            .map_err(|e| HuddleError::Video(e.to_string()))?;

        if let Err(err) = ready_rx
            .recv()
            .map_err(|e| HuddleError::Video(e.to_string()))
            .and_then(|r| r)
        {
            running.store(false, Ordering::Relaxed);
            let _ = worker.join();
            if let Some(h) = audio_worker {
                let _ = h.join();
            }
            return Err(err);
        }

        Ok(Self {
            running,
            latest_preview,
            thumbnails,
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
        let mut thumbnail_frames = self
            .thumbnails
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default();
        if let Some(last) = preview_frame.clone() {
            if thumbnail_frames.len() < 4 {
                thumbnail_frames.push(last);
            } else {
                thumbnail_frames[3] = last;
            }
        }
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
            thumbnail_frames,
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
    precomputed_preview: Option<VideoFrame>,
    state: &ClipEncodeState,
) {
    // Keep dimensions consistent across a clip (resize if the source window resized mid-clip).
    let (target_w, target_h) = {
        let Ok(mut d) = state.dims.lock() else { return };
        if *d == (0, 0) {
            *d = (w, h);
        }
        *d
    };
    let (enc_rgb, ew, eh, reused_preview): (Cow<'_, [u8]>, u32, u32, Option<VideoFrame>) =
        if (w, h) == (target_w, target_h) {
            (Cow::Borrowed(rgb), w, h, precomputed_preview)
        } else {
            let (resized, rw, rh) = resize_rgb_even(rgb, w, h, target_w, target_h);
            (Cow::Owned(resized), rw, rh, None)
        };
    if ew != target_w || eh != target_h {
        return;
    }
    let frame_idx = state.frames.lock().map(|g| g.len()).unwrap_or(0);
    let vf = reused_preview.unwrap_or_else(|| VideoFrame::from_bgra(ew, eh, rgb_to_bgra(&enc_rgb)));
    if let Ok(mut p) = state.preview.lock() {
        *p = Some(vf.clone());
    }
    // Sample candidate thumbnail keyframes at frame 0, ~2s, ~5s, and ~10s (`F-COLLAB-099`).
    if matches!(frame_idx, 0 | 24 | 60 | 120)
        && let Ok(mut t) = state.thumbs.lock()
        && t.len() < 4
    {
        t.push(vf);
    }
    if let Ok(annex_b) = enc.encode_rgb(&enc_rgb, ew as usize, eh as usize)
        && !annex_b.is_empty()
        && let Ok(mut list) = state.frames.lock()
    {
        list.push(EncodedH264Frame { annex_b });
    }
}
