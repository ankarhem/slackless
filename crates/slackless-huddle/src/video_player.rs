//! Native hardware-decoded in-app video player.
//!
//! On macOS this wraps `AVPlayer` + `AVPlayerItemVideoOutput` configured for zero-copy `420f`
//! IOSurface `CVPixelBuffer` frames ([`crate::MacVideoSurface`]) that GPUI renders directly via
//! Metal. On non-macOS targets, [`NativeVideoPlayer::open`] returns
//! [`crate::HuddleError::Unsupported`].

/// Snapshot of playback state returned by [`NativeVideoPlayer::state`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoPlayerState {
    pub playing: bool,
    pub muted: bool,
    pub ended: bool,
    pub failed: bool,
    pub position_secs: f64,
    pub duration_secs: f64,
    pub width: u32,
    pub height: u32,
    pub rate: f32,
    pub volume: f32,
}

impl Default for VideoPlayerState {
    fn default() -> Self {
        Self {
            playing: false,
            muted: false,
            ended: false,
            failed: false,
            position_secs: 0.0,
            duration_secs: 0.0,
            width: 0,
            height: 0,
            rate: 1.0,
            volume: 1.0,
        }
    }
}

/// Clamp volume into the supported `0.0..=1.0` range (defaulting non-finite values to `1.0`).
pub(crate) fn clamp_volume(volume: f32) -> f32 {
    if !volume.is_finite() {
        1.0
    } else {
        volume.clamp(0.0, 1.0)
    }
}

/// Clamp playback speed into the supported `0.5x..=2.0x` range (defaulting non-finite values to `1.0x`).
pub(crate) fn clamp_playback_rate(rate: f32) -> f32 {
    if !rate.is_finite() {
        1.0
    } else {
        rate.clamp(0.5, 2.0)
    }
}

/// Advance the playback speed preset cycle: `1.0 -> 1.5 -> 2.0 -> 1.0`.
pub(crate) fn next_playback_rate(current: f32) -> f32 {
    let cur = clamp_playback_rate(current);
    if cur < 1.25 {
        1.5
    } else if cur < 1.75 {
        2.0
    } else {
        1.0
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::CString;
    use std::path::Path;

    use crate::macos::MacVideoSurface;
    use crate::macos::ffi::{
        SlacklessVideoPlayer, slackless_video_player_copy_frame, slackless_video_player_destroy,
        slackless_video_player_open, slackless_video_player_open_url, slackless_video_player_pause,
        slackless_video_player_play, slackless_video_player_seek, slackless_video_player_set_muted,
        slackless_video_player_set_rate, slackless_video_player_set_volume,
        slackless_video_player_state,
    };
    use crate::{HuddleError, Result};

    use super::{VideoPlayerState, clamp_playback_rate, clamp_volume, next_playback_rate};

    /// Thread-safe handle to a native macOS `AVPlayer` + `AVPlayerItemVideoOutput` media player
    /// supporting local video/audio files as well as authenticated HTTP/HLS URLs.
    pub struct NativeVideoPlayer {
        handle: *mut SlacklessVideoPlayer,
    }

    // SAFETY: All FFI operations on `SlacklessVideoPlayer` synchronize via an internal
    // `pthread_mutex_t`, and teardown happens exclusively in `Drop`.
    unsafe impl Send for NativeVideoPlayer {}
    unsafe impl Sync for NativeVideoPlayer {}

    impl std::fmt::Debug for NativeVideoPlayer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("NativeVideoPlayer")
                .field("open", &(!self.handle.is_null()))
                .finish()
        }
    }

    impl NativeVideoPlayer {
        /// Open a local video or audio file for hardware-accelerated playback.
        pub fn open(path: &Path) -> Result<Self> {
            let path_str = path.to_str().ok_or_else(|| {
                HuddleError::Video(format!("video path is not valid UTF-8: {}", path.display()))
            })?;
            let c_path = CString::new(path_str).map_err(|_| {
                HuddleError::Video(format!(
                    "video path contains interior NUL byte: {}",
                    path.display()
                ))
            })?;
            // SAFETY: `c_path` is a valid NUL-terminated UTF-8 string.
            let handle = unsafe { slackless_video_player_open(c_path.as_ptr()) };
            if handle.is_null() {
                return Err(HuddleError::Video(format!(
                    "failed to open video file: {}",
                    path.display()
                )));
            }
            Ok(Self { handle })
        }

        /// Open an authenticated HTTP/HTTPS or HLS (`m3u8`) media URL with optional `Cookie` header.
        pub fn open_url(url: &str, cookie_header: Option<&str>) -> Result<Self> {
            let trimmed = url.trim();
            if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
                return Err(HuddleError::Video(format!(
                    "media URL must start with http:// or https://: {trimmed}"
                )));
            }
            let c_url = CString::new(trimmed).map_err(|_| {
                HuddleError::Video("media URL contains interior NUL byte".to_owned())
            })?;
            let c_cookie = match cookie_header.map(str::trim).filter(|s| !s.is_empty()) {
                Some(cookie) => Some(CString::new(cookie).map_err(|_| {
                    HuddleError::Video("cookie header contains interior NUL byte".to_owned())
                })?),
                None => None,
            };
            let cookie_ptr = c_cookie.as_ref().map_or(std::ptr::null(), |c| c.as_ptr());
            // SAFETY: `c_url` is a valid NUL-terminated UTF-8 string and `cookie_ptr` is either
            // NULL or a valid NUL-terminated UTF-8 string.
            let handle = unsafe { slackless_video_player_open_url(c_url.as_ptr(), cookie_ptr) };
            if handle.is_null() {
                return Err(HuddleError::Video(format!(
                    "failed to open media URL: {trimmed}"
                )));
            }
            Ok(Self { handle })
        }

        /// Start or resume playback (restarting from `0.0` if playback already reached the end).
        pub fn play(&self) {
            if !self.handle.is_null() {
                // SAFETY: `self.handle` is valid for the lifetime of `self`.
                unsafe { slackless_video_player_play(self.handle) };
            }
        }

        /// Pause playback.
        pub fn pause(&self) {
            if !self.handle.is_null() {
                // SAFETY: `self.handle` is valid for the lifetime of `self`.
                unsafe { slackless_video_player_pause(self.handle) };
            }
        }

        /// Toggle between playing and paused states.
        pub fn toggle_play(&self) {
            if self.state().playing {
                self.pause();
            } else {
                self.play();
            }
        }

        /// Mute or unmute the media's audio track.
        pub fn set_muted(&self, muted: bool) {
            if !self.handle.is_null() {
                // SAFETY: `self.handle` is valid for the lifetime of `self`.
                unsafe { slackless_video_player_set_muted(self.handle, muted) };
            }
        }

        /// Set playback volume (`0.0..=1.0`, unmuting when raised above zero).
        pub fn set_volume(&self, volume: f32) {
            if !self.handle.is_null() {
                let clamped = clamp_volume(volume);
                // SAFETY: `self.handle` is valid for the lifetime of `self`.
                unsafe { slackless_video_player_set_volume(self.handle, clamped) };
            }
        }

        /// Adjust playback volume by `delta` (`-1.0..=1.0`) and return the resulting clamped level.
        pub fn adjust_volume(&self, delta: f32) -> f32 {
            let st = self.state();
            let base = if st.muted && delta > 0.0 {
                0.0
            } else {
                st.volume
            };
            let next = clamp_volume(base + delta);
            self.set_volume(next);
            next
        }

        /// Set playback rate (clamped to `0.5..=2.0`, preserving audio pitch via time-domain
        /// scaling). Takes effect immediately if playing and is preserved across pause/resume.
        pub fn set_rate(&self, rate: f32) {
            if !self.handle.is_null() {
                let clamped = clamp_playback_rate(rate);
                // SAFETY: `self.handle` is valid for the lifetime of `self`.
                unsafe { slackless_video_player_set_rate(self.handle, clamped) };
            }
        }

        /// Cycle playback speed through `1.0x -> 1.5x -> 2.0x -> 1.0x` and return the new rate.
        pub fn cycle_rate(&self) -> f32 {
            let next = next_playback_rate(self.state().rate);
            self.set_rate(next);
            next
        }

        /// Seek to `secs` seconds with zero-tolerance frame accuracy.
        pub fn seek(&self, secs: f64) {
            if !self.handle.is_null() {
                // SAFETY: `self.handle` is valid for the lifetime of `self`.
                unsafe { slackless_video_player_seek(self.handle, secs) };
            }
        }

        /// Read a consistent snapshot of playback position, duration, dimensions, speed, and status flags.
        pub fn state(&self) -> VideoPlayerState {
            let mut st = VideoPlayerState::default();
            if self.handle.is_null() {
                return st;
            }
            // SAFETY: `self.handle` is valid and all out-pointers reference local stack variables.
            unsafe {
                slackless_video_player_state(
                    self.handle,
                    &mut st.playing,
                    &mut st.muted,
                    &mut st.ended,
                    &mut st.position_secs,
                    &mut st.duration_secs,
                    &mut st.width,
                    &mut st.height,
                    &mut st.failed,
                    &mut st.rate,
                    &mut st.volume,
                );
            }
            st
        }

        /// Copy the latest zero-copy `420f` IOSurface frame for rendering, or `None` if no frame
        /// has been decoded yet (or if the media has no video track).
        pub fn copy_frame(&self) -> Option<MacVideoSurface> {
            if self.handle.is_null() {
                return None;
            }
            let mut w = 0u32;
            let mut h = 0u32;
            // SAFETY: `self.handle` is valid; `slackless_video_player_copy_frame` returns a +1
            // retained `CVPixelBufferRef` (or null) whose ownership transfers to `MacVideoSurface`.
            let pb = unsafe { slackless_video_player_copy_frame(self.handle, &mut w, &mut h) };
            unsafe { MacVideoSurface::from_retained(pb, w, h) }
        }
    }

    impl Drop for NativeVideoPlayer {
        fn drop(&mut self) {
            if !self.handle.is_null() {
                // SAFETY: `self.handle` is destroyed exactly once here.
                unsafe { slackless_video_player_destroy(self.handle) };
                self.handle = std::ptr::null_mut();
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use std::path::Path;

    use crate::{HuddleError, Result};

    use super::VideoPlayerState;

    /// Stub native video player on non-macOS targets.
    #[derive(Debug)]
    pub struct NativeVideoPlayer {
        _private: (),
    }

    impl NativeVideoPlayer {
        pub fn open(_path: &Path) -> Result<Self> {
            Err(HuddleError::Unsupported(
                "native in-app video player is only supported on macOS".to_owned(),
            ))
        }

        pub fn open_url(_url: &str, _cookie_header: Option<&str>) -> Result<Self> {
            Err(HuddleError::Unsupported(
                "native in-app video player is only supported on macOS".to_owned(),
            ))
        }

        pub fn play(&self) {}

        pub fn pause(&self) {}

        pub fn toggle_play(&self) {}

        pub fn set_muted(&self, _muted: bool) {}

        pub fn set_volume(&self, _volume: f32) {}

        pub fn adjust_volume(&self, _delta: f32) -> f32 {
            1.0
        }

        pub fn set_rate(&self, _rate: f32) {}

        pub fn cycle_rate(&self) -> f32 {
            1.0
        }

        pub fn seek(&self, _secs: f64) {}

        pub fn state(&self) -> VideoPlayerState {
            VideoPlayerState::default()
        }
    }
}

pub use imp::NativeVideoPlayer;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HuddleError;

    #[test]
    fn open_missing_file_returns_error() {
        let missing = std::env::temp_dir().join(format!(
            "slackless-video-player-missing-{}.mp4",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&missing);
        let err = NativeVideoPlayer::open(&missing).expect_err("missing file must fail");
        #[cfg(target_os = "macos")]
        assert!(matches!(err, HuddleError::Video(_)), "{err:?}");
        #[cfg(not(target_os = "macos"))]
        assert!(matches!(err, HuddleError::Unsupported(_)), "{err:?}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn open_poll_control_and_drop_synthetic_h264_mp4() {
        use crate::clips::mux_h264_mp4;
        use crate::video::H264VideoEncoder;
        use std::time::{Duration, Instant};

        let w = 160usize;
        let h = 96usize;
        let fps = 12u32;
        let mut enc = H264VideoEncoder::new_screen().expect("openh264 screen encoder");
        let mut frames = Vec::new();
        for f in 0..12u8 {
            let mut rgb = vec![0u8; w * h * 3];
            for y in 0..h {
                for x in 0..w {
                    let off = (y * w + x) * 3;
                    rgb[off] = ((x * 255) / w) as u8;
                    rgb[off + 1] = ((y * 255) / h) as u8;
                    rgb[off + 2] = f.saturating_mul(20);
                }
            }
            let bs = enc.encode_rgb(&rgb, w, h).expect("encode H.264 frame");
            frames.push(bs);
        }
        let mp4_bytes = mux_h264_mp4(w as u32, h as u32, fps, &frames).expect("mux MP4");
        let path = std::env::temp_dir().join(format!(
            "slackless-video-player-test-{}.mp4",
            std::process::id()
        ));
        std::fs::write(&path, &mp4_bytes).expect("write temp MP4");

        let player = NativeVideoPlayer::open(&path).expect("open synthetic MP4");

        // Poll briefly for AVAsset/AVPlayerItem metadata readiness.
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut st = player.state();
        while (st.width == 0 || st.height == 0 || st.duration_secs <= 0.0)
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(25));
            st = player.state();
        }

        assert!(!st.failed, "player reported failed state: {st:?}");
        assert_eq!(st.width, w as u32, "expected natural width {w}, got {st:?}");
        assert_eq!(
            st.height, h as u32,
            "expected natural height {h}, got {st:?}"
        );
        assert!(
            st.duration_secs > 0.5 && st.duration_secs < 2.0,
            "expected ~1.0s duration, got {st:?}"
        );

        player.set_muted(true);
        assert!(player.state().muted);

        // Volume controls clamp and unmute when raised from muted.
        player.set_volume(0.4);
        assert!(!player.state().muted);
        assert!((player.state().volume - 0.4).abs() < 1e-4);
        assert!((player.adjust_volume(0.2) - 0.6).abs() < 1e-4);
        assert!((player.state().volume - 0.6).abs() < 1e-4);
        assert!((player.adjust_volume(1.0) - 1.0).abs() < 1e-4);
        assert!((player.adjust_volume(-2.0) - 0.0).abs() < 1e-4);
        player.set_muted(true);
        assert!(player.state().muted);
        assert!((player.adjust_volume(0.1) - 0.1).abs() < 1e-4);
        assert!(!player.state().muted);
        player.set_muted(false);
        assert!(!player.state().muted);

        player.play();
        let play_deadline = Instant::now() + Duration::from_secs(2);
        let mut got_frame = None;
        while Instant::now() < play_deadline {
            if let Some(surface) = player.copy_frame() {
                got_frame = Some(surface);
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        if let Some(surface) = got_frame {
            assert_eq!(surface.width(), w as u32);
            assert_eq!(surface.height(), h as u32);
        }

        player.pause();
        assert!(!player.state().playing);

        player.toggle_play();
        assert!(player.state().playing);
        player.toggle_play();
        assert!(!player.state().playing);

        player.seek(0.25);
        let _ = player.copy_frame();

        drop(player);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn playback_rate_clamping_and_cycling() {
        assert_eq!(VideoPlayerState::default().rate, 1.0);
        assert_eq!(clamp_playback_rate(0.2), 0.5);
        assert_eq!(clamp_playback_rate(3.5), 2.0);
        assert_eq!(clamp_playback_rate(f32::NAN), 1.0);
        assert_eq!(clamp_playback_rate(1.5), 1.5);

        assert_eq!(next_playback_rate(1.0), 1.5);
        assert_eq!(next_playback_rate(1.5), 2.0);
        assert_eq!(next_playback_rate(2.0), 1.0);
        assert_eq!(next_playback_rate(0.5), 1.5);

        assert_eq!(VideoPlayerState::default().volume, 1.0);
        assert_eq!(clamp_volume(-0.25), 0.0);
        assert_eq!(clamp_volume(1.5), 1.0);
        assert_eq!(clamp_volume(f32::NAN), 1.0);
        assert_eq!(clamp_volume(0.6), 0.6);
    }

    #[test]
    fn open_url_rejects_invalid_scheme() {
        let err = NativeVideoPlayer::open_url("file:///tmp/test.mp4", None)
            .expect_err("non-http URL must fail");
        #[cfg(target_os = "macos")]
        assert!(matches!(err, HuddleError::Video(_)), "{err:?}");
        #[cfg(not(target_os = "macos"))]
        assert!(matches!(err, HuddleError::Unsupported(_)), "{err:?}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn open_poll_control_and_speed_on_audio_only_wav() {
        use std::time::{Duration, Instant};

        // Synthesize a 1.5s 16 kHz mono 440 Hz sine wave PCM16 WAV file.
        let sample_rate = 16_000u32;
        let samples: Vec<i16> = (0..(sample_rate as usize * 3 / 2))
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                ((t * 440.0 * std::f32::consts::TAU).sin() * 12_000.0) as i16
            })
            .collect();
        let wav_bytes = crate::clips::encode_wav_pcm16(&samples, sample_rate, 1);
        let path = std::env::temp_dir().join(format!(
            "slackless-audio-player-test-{}.wav",
            std::process::id()
        ));
        std::fs::write(&path, &wav_bytes).expect("write synthetic WAV");

        let player = NativeVideoPlayer::open(&path).expect("open audio-only WAV");

        // Poll briefly for AVPlayerItem duration readiness.
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut st = player.state();
        while st.duration_secs <= 0.0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
            st = player.state();
        }

        assert!(!st.failed, "audio player reported failed state: {st:?}");
        assert_eq!(st.width, 0, "audio-only WAV must have width == 0: {st:?}");
        assert_eq!(st.height, 0, "audio-only WAV must have height == 0: {st:?}");
        assert!(
            st.duration_secs > 1.0 && st.duration_secs < 2.0,
            "expected ~1.5s duration, got {st:?}"
        );
        assert_eq!(st.rate, 1.0);
        assert!(
            player.copy_frame().is_none(),
            "audio-only media must not produce video frames"
        );

        player.set_muted(true);
        assert!(player.state().muted);

        // Volume controls clamp and unmute when raised from muted.
        player.set_volume(0.4);
        assert!(!player.state().muted);
        assert!((player.state().volume - 0.4).abs() < 1e-4);
        assert!((player.adjust_volume(0.2) - 0.6).abs() < 1e-4);
        assert!((player.state().volume - 0.6).abs() < 1e-4);
        assert!((player.adjust_volume(1.0) - 1.0).abs() < 1e-4);
        assert!((player.adjust_volume(-2.0) - 0.0).abs() < 1e-4);
        player.set_muted(true);
        assert!(player.state().muted);
        assert!((player.adjust_volume(0.1) - 0.1).abs() < 1e-4);
        assert!(!player.state().muted);

        // Speed controls work while paused and persist across play/pause.
        player.set_rate(1.5);
        assert!((player.state().rate - 1.5).abs() < 1e-4);
        assert_eq!(player.cycle_rate(), 2.0);
        assert!((player.state().rate - 2.0).abs() < 1e-4);
        assert_eq!(player.cycle_rate(), 1.0);
        assert!((player.state().rate - 1.0).abs() < 1e-4);
        assert_eq!(player.cycle_rate(), 1.5);
        assert!((player.state().rate - 1.5).abs() < 1e-4);

        player.play();
        assert!(player.state().playing);
        assert!((player.state().rate - 1.5).abs() < 1e-4);

        // Changing rate while playing keeps playback active at the new rate.
        player.set_rate(2.0);
        let playing_state = player.state();
        assert!(playing_state.playing);
        assert!((playing_state.rate - 2.0).abs() < 1e-4);
        assert!(player.copy_frame().is_none());

        player.seek(0.5);
        player.pause();
        assert!(!player.state().playing);
        assert!((player.state().rate - 2.0).abs() < 1e-4);

        drop(player);
        let _ = std::fs::remove_file(&path);
    }
}
