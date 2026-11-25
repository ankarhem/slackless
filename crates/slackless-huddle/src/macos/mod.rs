//! Safe Rust wrappers for macOS hardware media acceleration compiled from `native/macos/*.m`.

#![cfg(target_os = "macos")]

pub mod audio;
pub mod camera;
pub mod codecs;
pub(crate) mod ffi;
pub mod overlay;
pub mod pixel;
pub mod screen;

pub use audio::{
    CoreAudioDeviceMonitor, MacRoutedAudioLoop, MacSystemAudioTap, MacVoiceProcessingIo,
    measure_audio_file_rms, play_routed_audio_oneshot, supports_native_system_audio_tap,
};
pub use camera::{AvVtCameraCapture, list_macos_cameras};
pub use codecs::{VtAv1Decoder, VtH264Decoder};
pub use ffi::{OverlayCursorFfi, OverlaySegmentFfi};
pub use overlay::{
    OverlayMouseKind, configure_floating_panel, hide_desktop_draw_overlay,
    set_desktop_draw_pen_mode, update_desktop_draw_overlay,
};
pub use pixel::{
    MacVideoSurface, has_screen_capture_permission, open_screen_capture_settings,
    request_screen_capture_permission, supports_hw_av1_decode, supports_hw_h264_decode,
};
pub use screen::{ScVtScreenCapture, capture_macos_thumbnails, parse_screen_source_id};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::{CameraDeviceInfo, H264VideoEncoder};
    use std::ffi::{c_char, c_void};

    #[test]
    fn videotoolbox_decodes_openh264_annex_b_to_zero_copy_surface_and_bgra() {
        let mut enc = H264VideoEncoder::new_screen().expect("openh264 screen encoder");
        let w = 160usize;
        let h = 96usize;
        let mut rgb = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let off = (y * w + x) * 3;
                rgb[off] = ((x * 255) / w) as u8;
                rgb[off + 1] = ((y * 255) / h) as u8;
                rgb[off + 2] = 180;
            }
        }
        let bitstream = enc.encode_rgb(&rgb, w, h).expect("encode keyframe");
        assert!(!bitstream.is_empty());

        let mut vt = VtH264Decoder::new().expect("VideoToolbox decoder");
        let decoded = vt
            .decode_annex_b(&bitstream)
            .expect("VideoToolbox should decode H.264 Annex-B keyframe");
        assert_eq!(decoded.width, w as u32);
        assert_eq!(decoded.height, h as u32);
        assert!(
            decoded.mac_surface().is_some(),
            "should return zero-copy MacVideoSurface"
        );
        let (bw, bh, bgra) = decoded
            .to_bgra_fit(0, 0)
            .expect("VTPixelTransferSession / vImage conversion to BGRA should succeed");
        assert_eq!((bw, bh), (w as u32, h as u32));
        assert_eq!(bgra.len(), w * h * 4);

        let (sw, sh, small_bgra) = decoded
            .to_bgra_fit(80, 48)
            .expect("VTPixelTransferSession hardware downscale to BGRA should succeed");
        assert_eq!((sw, sh), (80, 48));
        assert_eq!(small_bgra.len(), 80 * 48 * 4);
        let tr = ((2 * sw as usize) + (sw as usize - 3)) * 4;
        assert!(
            small_bgra[tr + 2] > 180 && small_bgra[tr + 1] < 60 && small_bgra[tr] > 110,
            "expected BGRA channel order at top-right, got {:?}",
            &small_bgra[tr..tr + 4]
        );
        let bl = (((sh as usize - 3) * sw as usize) + 2) * 4;
        assert!(
            small_bgra[bl + 1] > 180 && small_bgra[bl + 2] < 60 && small_bgra[bl] > 110,
            "expected BGRA channel order at bottom-left, got {:?}",
            &small_bgra[bl..bl + 4]
        );
    }

    #[test]
    fn hw_av1_probe_matches_vt_av1_decoder_availability() {
        let supported = supports_hw_av1_decode();
        let dec = VtAv1Decoder::new();
        assert_eq!(dec.is_ok(), supported);
    }

    #[test]
    fn draw_overlay_handles_empty_and_invalid_sources_safely() {
        update_desktop_draw_overlay("invalid:abc", &[], &[]);
        update_desktop_draw_overlay("monitor:notanumber", &[], &[]);
        update_desktop_draw_overlay("window:notanumber", &[], &[]);
        hide_desktop_draw_overlay();
    }

    #[test]
    fn parse_screen_source_id_handles_valid_and_invalid_inputs_consistently() {
        assert_eq!(parse_screen_source_id("monitor:0"), Some((true, 0)));
        assert_eq!(parse_screen_source_id("monitor:42"), Some((true, 42)));
        assert_eq!(
            parse_screen_source_id(&format!("monitor:{}", u32::MAX)),
            Some((true, u32::MAX))
        );
        assert_eq!(parse_screen_source_id("window:12345"), Some((false, 12345)));
        assert_eq!(parse_screen_source_id("window:0"), Some((false, 0)));

        for bad in [
            "",
            "monitor:",
            "window:",
            "monitor:-1",
            "window:abc",
            "monitor:4294967296",
            "MONITOR:1",
            "display:1",
            "monitor:1 ",
            " window:1",
        ] {
            assert_eq!(
                parse_screen_source_id(bad),
                None,
                "expected None for {bad:?}"
            );
        }
    }

    #[test]
    fn measure_audio_file_rms_measures_wav_tone_and_rejects_silence_or_missing() {
        let dir = std::env::temp_dir();
        let pid = std::process::id();

        let tone_path = dir.join(format!("slackless-test-tone-{pid}.wav"));
        let tone_pcm: Vec<i16> = (0..12_000)
            .map(|i| {
                ((i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 16_384.0) as i16
            })
            .collect();
        let tone_wav = crate::clips::encode_wav_pcm16(&tone_pcm, 48_000, 1);
        std::fs::write(&tone_path, &tone_wav).unwrap();
        let rms = measure_audio_file_rms(&tone_path).expect("tone WAV should have positive RMS");
        let _ = std::fs::remove_file(&tone_path);
        assert!(
            rms > 0.1 && rms < 0.8,
            "expected reasonable RMS for 440 Hz sine, got {rms}"
        );

        let silent_path = dir.join(format!("slackless-test-silence-{pid}.wav"));
        let silent_wav = crate::clips::encode_wav_pcm16(&vec![0i16; 4_800], 48_000, 1);
        std::fs::write(&silent_path, &silent_wav).unwrap();
        let silent_rms = measure_audio_file_rms(&silent_path);
        let _ = std::fs::remove_file(&silent_path);
        assert!(silent_rms.is_none());

        let missing = dir.join(format!("slackless-nonexistent-{pid}.wav"));
        assert!(measure_audio_file_rms(&missing).is_none());
    }

    #[test]
    fn camera_enum_callback_normalizes_null_blank_invalid_utf8_and_trimmed_names() {
        let mut raw: Vec<(usize, Option<String>)> = Vec::new();
        let ctx = (&mut raw as *mut Vec<(usize, Option<String>)>).cast::<c_void>();

        let valid = std::ffi::CString::new("  FaceTime HD Camera  ").unwrap();
        let blank = std::ffi::CString::new("   ").unwrap();
        let invalid_utf8: [c_char; 2] = [-1, 0];

        unsafe {
            camera::on_cam_enum_cb(std::ptr::null_mut(), 99, valid.as_ptr());
            camera::on_cam_enum_cb(ctx, 0, valid.as_ptr());
            camera::on_cam_enum_cb(ctx, 1, std::ptr::null());
            camera::on_cam_enum_cb(ctx, 2, blank.as_ptr());
            camera::on_cam_enum_cb(ctx, 3, invalid_utf8.as_ptr());
        }

        let out = crate::video::normalize_camera_devices(raw);
        assert_eq!(
            out,
            vec![
                CameraDeviceInfo {
                    index: 0,
                    name: "FaceTime HD Camera".to_owned(),
                },
                CameraDeviceInfo {
                    index: 1,
                    name: "Camera 2".to_owned(),
                },
                CameraDeviceInfo {
                    index: 2,
                    name: "Camera 3".to_owned(),
                },
                CameraDeviceInfo {
                    index: 3,
                    name: "Camera 4".to_owned(),
                },
            ]
        );
    }
}
