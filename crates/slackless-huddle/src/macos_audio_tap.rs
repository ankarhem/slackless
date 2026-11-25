//! Zero-driver macOS 14.2+ system audio capture backed by the native ARC Objective-C module
//! (`native/macos/audio_tap.m`) and a lock-free SPSC PCM ring buffer.

#![cfg(target_os = "macos")]

use std::sync::Arc;

use crate::Result;
use crate::audio::ring::SpscRing;
use crate::macos::audio::{
    MacSystemAudioTap, supports_native_system_audio_tap as native_supported,
};

/// Returns `true` when macOS 14.2+ CoreAudio Process Taps (`CATapDescription`) are available at runtime.
pub fn supports_native_system_audio_tap() -> bool {
    native_supported()
}

/// Live macOS 14.2+ CoreAudio Process Tap streaming stereo `i16` PCM into a lock-free SPSC ring.
pub struct SystemAudioTap {
    _inner: MacSystemAudioTap,
}

impl SystemAudioTap {
    /// Create a global stereo CoreAudio Process Tap and start streaming PCM into `ring`.
    pub(crate) fn start(ring: Arc<SpscRing<i16>>) -> Result<Self> {
        let inner = MacSystemAudioTap::start(ring)?;
        Ok(Self { _inner: inner })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_detects_process_tap_class_without_panicking() {
        let _supported = supports_native_system_audio_tap();
    }
}
