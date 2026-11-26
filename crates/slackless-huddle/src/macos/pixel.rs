//! macOS runtime capability probes, screen capture permissions, and zero-copy `MacVideoSurface`.

use std::ffi::c_void;
use std::fmt;
use std::ptr::NonNull;

use super::ffi::{
    slackless_cvpb_dimensions, slackless_cvpb_release, slackless_cvpb_retain,
    slackless_cvpb_to_bgra, slackless_free_buf, slackless_has_screen_capture_permission,
    slackless_macos_supports_hw_av1_decode, slackless_macos_supports_hw_h264_decode,
    slackless_request_screen_capture_permission,
};

/// Returns `true` if macOS Screen Recording permission has been granted to this app bundle.
pub fn has_screen_capture_permission() -> bool {
    slackless_has_screen_capture_permission()
}

/// Triggers the macOS Screen Recording permission prompt (or registers the app in System Settings).
pub fn request_screen_capture_permission() -> bool {
    slackless_request_screen_capture_permission()
}

/// Opens macOS System Settings directly to Privacy & Security -> Screen & System Audio Recording.
pub fn open_screen_capture_settings() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
        .spawn();
}

/// Runtime probe: does this Mac support hardware H.264 decoding via VideoToolbox?
pub fn supports_hw_h264_decode() -> bool {
    slackless_macos_supports_hw_h264_decode()
}

/// Runtime probe: does this Mac support hardware AV1 decoding via VideoToolbox (Apple Silicon M3+
/// on macOS 14+)? Safely returns `false` on M1/M2/Intel Macs.
pub fn supports_hw_av1_decode() -> bool {
    slackless_macos_supports_hw_av1_decode()
}

/// Zero-copy retained handle to a macOS `IOSurface`-backed `420f` `CVPixelBuffer`.
pub struct MacVideoSurface {
    pb: NonNull<c_void>,
    width: u32,
    height: u32,
}

// SAFETY: An immutable `CVPixelBuffer` backed by an `IOSurface` is reference-counted via
// `CFRetain`/`CFRelease` and safe to pass across threads for Metal read sampling or read-only lock.
unsafe impl Send for MacVideoSurface {}
// SAFETY: `&self` methods only retain the buffer or read it under a read-only lock (the shared
// pixel-transfer session in `slackless_cvpb_to_bgra` sits behind a mutex), which CoreVideo allows
// from several threads at once.
unsafe impl Sync for MacVideoSurface {}

impl fmt::Debug for MacVideoSurface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MacVideoSurface")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

impl MacVideoSurface {
    /// Wrap an already-retained `CVPixelBufferRef` (transfers +1 retain ownership); `None` for
    /// null.
    ///
    /// # Safety
    /// `pb` must be null or a `CVPixelBufferRef` the caller holds a +1 retain on.
    pub(crate) unsafe fn from_retained(pb: *mut c_void, width: u32, height: u32) -> Option<Self> {
        let pb = NonNull::new(pb)?;
        let (mut w, mut h) = (width, height);
        if w == 0 || h == 0 {
            // SAFETY: `pb` is a retained `CVPixelBufferRef` (see `# Safety`); `w`/`h` are locals.
            unsafe {
                slackless_cvpb_dimensions(pb.as_ptr(), &mut w, &mut h);
            }
        }
        Some(Self {
            pb,
            width: w,
            height: h,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Returns a newly `CFRetain`ed `CVPixelBufferRef` pointer for GPUI `CFRetained::from_raw`.
    /// Caller owns the +1 retain count and MUST release it (e.g. via `CFRetained::from_raw`).
    pub fn retain_cv_pixel_buffer_ptr(&self) -> *mut c_void {
        // SAFETY: `pb` holds a retain for as long as `self` lives.
        unsafe { slackless_cvpb_retain(self.pb.as_ptr()) }
    }

    /// Convert (and optionally high-quality downscale) this GPU surface to tightly-packed BGRA8888
    /// using Apple's `VTPixelTransferSession` / NEON `vImage`. Pass `0, 0` for native dimensions.
    pub fn to_bgra_fit(&self, max_w: u32, max_h: u32) -> Option<(u32, u32, Vec<u8>)> {
        let mut ptr: *mut u8 = std::ptr::null_mut();
        let mut len: usize = 0;
        let mut out_w: u32 = 0;
        let mut out_h: u32 = 0;
        // SAFETY: `pb` holds a retain for as long as `self` lives; the out-pointers are locals.
        let ok = unsafe {
            slackless_cvpb_to_bgra(
                self.pb.as_ptr(),
                max_w,
                max_h,
                &mut ptr,
                &mut len,
                &mut out_w,
                &mut out_h,
            )
        };
        if !ok || ptr.is_null() || len == 0 {
            if !ptr.is_null() {
                // SAFETY: `ptr` was allocated by `slackless_cvpb_to_bgra` and is freed once.
                unsafe { slackless_free_buf(ptr) };
            }
            return None;
        }
        // SAFETY: on success `ptr` holds `len` initialized bytes until the free below.
        let vec = unsafe { std::slice::from_raw_parts(ptr, len).to_vec() };
        // SAFETY: `ptr` was allocated by `slackless_cvpb_to_bgra` and is freed once.
        unsafe { slackless_free_buf(ptr) };
        Some((out_w, out_h, vec))
    }
}

impl Drop for MacVideoSurface {
    fn drop(&mut self) {
        // SAFETY: releases, once, the +1 retain this surface took over in `from_retained`.
        unsafe { slackless_cvpb_release(self.pb.as_ptr()) };
    }
}
