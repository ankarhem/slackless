//! Native hardware-decoded in-app video player.
//!
//! On macOS this wraps `AVPlayer` + `AVPlayerItemVideoOutput` configured for zero-copy `420f`
//! IOSurface `CVPixelBuffer` frames ([`crate::MacVideoSurface`]) that GPUI renders directly via
//! Metal. On non-macOS targets, [`NativeVideoPlayer::open`] returns [`HuddleError::Unsupported`].

use std::path::Path;

use crate::{HuddleError, Result};

/// Snapshot of playback state returned by [`NativeVideoPlayer::state`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct VideoPlayerState {
    pub playing: bool,
    pub muted: bool,
    pub ended: bool,
    pub failed: bool,
    pub position_secs: f64,
    pub duration_secs: f64,
    pub width: u32,
    pub height: u32,
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::CString;
    use std::path::Path;

    use crate::macos::MacVideoSurface;
    use crate::macos::ffi::{
        SlacklessVideoPlayer, slackless_video_player_copy_frame, slackless_video_player_destroy,
        slackless_video_player_open, slackless_video_player_pause, slackless_video_player_play,
        slackless_video_player_seek, slackless_video_player_set_muted,
        slackless_video_player_state,
    };
    use crate::{HuddleError, Result};

    use super::VideoPlayerState;

    /// Thread-safe handle to a native macOS `AVPlayer` + `AVPlayerItemVideoOutput` video player.
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
        /// Open a local video file for hardware-accelerated playback.
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

        /// Mute or unmute the video's audio track.
        pub fn set_muted(&self, muted: bool) {
            if !self.handle.is_null() {
                // SAFETY: `self.handle` is valid for the lifetime of `self`.
                unsafe { slackless_video_player_set_muted(self.handle, muted) };
            }
        }

        /// Seek to `secs` seconds with zero-tolerance frame accuracy.
        pub fn seek(&self, secs: f64) {
            if !self.handle.is_null() {
                // SAFETY: `self.handle` is valid for the lifetime of `self`.
                unsafe { slackless_video_player_seek(self.handle, secs) };
            }
        }

        /// Read a consistent snapshot of playback position, duration, dimensions, and status flags.
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
                );
            }
            st
        }

        /// Copy the latest zero-copy `420f` IOSurface frame for rendering, or `None` if no frame
        /// has been decoded yet.
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

        pub fn play(&self) {}

        pub fn pause(&self) {}

        pub fn toggle_play(&self) {}

        pub fn set_muted(&self, _muted: bool) {}

        pub fn seek(&self, _secs: f64) {}

        pub fn state(&self) -> VideoPlayerState {
            VideoPlayerState::default()
        }
    }
}

pub use imp::NativeVideoPlayer;

/// Open a local video file with [`NativeVideoPlayer::open`].
pub fn open_video_player(path: &Path) -> Result<NativeVideoPlayer> {
    if !path.is_file() {
        return Err(HuddleError::Video(format!(
            "video file not found: {}",
            path.display()
        )));
    }
    NativeVideoPlayer::open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
