use super::audio::{
    downsample_waveform, encode_wav_pcm16, parse_adts_frames, transcode_wav_to_aac_adts,
};
use super::video::{CapturedRgbFrame, ClipFrameSource, video_frame_to_clip_pair};
use super::*;
use crate::HuddleError;
use crate::video::{CAMERA_MAX_H, CAMERA_MAX_W, H264VideoEncoder, VideoFrame, split_annex_b_nals};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

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
        .map(|i| ((i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 12_000.0) as i16)
        .collect();
    let wav = encode_wav_pcm16(&pcm, 48_000, 1);
    let adts = transcode_wav_to_aac_adts(&wav).expect("afconvert adts");
    let track = parse_adts_frames(&adts).expect("parse adts");
    assert_eq!(track.sample_rate, 48_000);
    assert_eq!(track.channels, 1);
    assert!(track.frames.len() >= 10);
}

#[derive(Debug)]
struct Mp4Box<'a> {
    fourcc: [u8; 4],
    offset: usize,
    size: usize,
    payload: &'a [u8],
}

fn parse_mp4_boxes(data: &[u8]) -> Vec<Mp4Box<'_>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < data.len() {
        assert!(pos + 8 <= data.len(), "truncated box header at {pos}");
        let size = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        assert!(size >= 8, "invalid box size {size} at {pos}");
        assert!(pos + size <= data.len(), "box overflows container at {pos}");
        let fourcc: [u8; 4] = data[pos + 4..pos + 8].try_into().unwrap();
        out.push(Mp4Box {
            fourcc,
            offset: pos,
            size,
            payload: &data[pos + 8..pos + size],
        });
        pos += size;
    }
    assert_eq!(
        pos,
        data.len(),
        "box sizes must sum to exact container size"
    );
    out
}

fn find_box<'a>(boxes: &'a [Mp4Box<'a>], fourcc: &[u8; 4]) -> &'a Mp4Box<'a> {
    boxes
        .iter()
        .find(|b| &b.fourcc == fourcc)
        .unwrap_or_else(|| panic!("missing box {:?}", std::str::from_utf8(fourcc)))
}

fn read_u32_be(b: &[u8], off: usize) -> u32 {
    u32::from_be_bytes(b[off..off + 4].try_into().unwrap())
}

fn synthetic_h264_frames() -> (Vec<u8>, Vec<u8>, Vec<Vec<u8>>) {
    let sps = vec![0x67, 0x42, 0xE0, 0x1F, 0x8D, 0x68];
    let pps = vec![0x68, 0xCE, 0x38, 0x80];
    let aud = vec![0x09, 0x10];
    let idr = vec![0x65, 0x88, 0x84, 0x21, 0xA0];
    let p_slice = vec![0x41, 0x9A, 0x24, 0x6C];

    // Frame 1: 4-byte SPS + 3-byte PPS + 4-byte AUD (stripped) + 4-byte IDR.
    let mut f1 = Vec::new();
    f1.extend_from_slice(&[0, 0, 0, 1]);
    f1.extend_from_slice(&sps);
    f1.extend_from_slice(&[0, 0, 1]);
    f1.extend_from_slice(&pps);
    f1.extend_from_slice(&[0, 0, 0, 1]);
    f1.extend_from_slice(&aud);
    f1.extend_from_slice(&[0, 0, 0, 1]);
    f1.extend_from_slice(&idr);

    // Frame 2: P-slice (non-IDR).
    let mut f2 = Vec::new();
    f2.extend_from_slice(&[0, 0, 1]);
    f2.extend_from_slice(&p_slice);

    // Frame 3: Second IDR.
    let mut f3 = Vec::new();
    f3.extend_from_slice(&[0, 0, 0, 1]);
    f3.extend_from_slice(&idr);

    (sps, pps, vec![f1, f2, f3])
}

#[test]
fn mp4_structural_validator_checks_boxes_offsets_stsz_avcc_stss_esds_and_next_track_id() {
    let (sps, pps, frames) = synthetic_h264_frames();

    // 1. Video-only MP4 and empty-audio-track MP4 both set next_track_id = 2 and emit 1 trak.
    let empty_audio = AacAudioTrack {
        sample_rate: 48_000,
        channels: 1,
        audio_specific_config: [0x11, 0x88],
        frames: Vec::new(),
    };
    for mp4 in [
        mux_h264_mp4(320, 240, 15, &frames).unwrap(),
        mux_h264_aac_mp4(320, 240, 15, &frames, Some(&empty_audio)).unwrap(),
    ] {
        let top = parse_mp4_boxes(&mp4);
        assert_eq!(top.len(), 3);
        assert_eq!(&top[0].fourcc, b"ftyp");
        assert_eq!(&top[1].fourcc, b"mdat");
        assert_eq!(&top[2].fourcc, b"moov");
        let moov_kids = parse_mp4_boxes(top[2].payload);
        let mvhd = find_box(&moov_kids, b"mvhd");
        // FullBox(4) + 96 bytes mvhd v0 body; next_track_id is the final u32.
        let next_track_id = read_u32_be(mvhd.payload, mvhd.payload.len() - 4);
        assert_eq!(
            next_track_id, 2,
            "next_track_id must be 2 without audio frames"
        );
        let traks: Vec<_> = moov_kids.iter().filter(|b| &b.fourcc == b"trak").collect();
        assert_eq!(traks.len(), 1);
    }

    // 2. Dual-track MP4: verify every box, offset, stsz sum, avcC SPS/PPS, stss, esds ASC.
    let audio = AacAudioTrack {
        sample_rate: 48_000,
        channels: 2,
        audio_specific_config: [0x11, 0x90],
        frames: vec![vec![0xDE, 0xAD, 0xBE, 0xEF], vec![0xCA, 0xFE, 0xBA]],
    };
    let mp4 = mux_h264_aac_mp4(320, 240, 15, &frames, Some(&audio)).unwrap();
    let top = parse_mp4_boxes(&mp4);
    assert_eq!(top.len(), 3);
    let ftyp = &top[0];
    let mdat = &top[1];
    let moov = &top[2];
    assert_eq!(mdat.offset, ftyp.size);

    let moov_kids = parse_mp4_boxes(moov.payload);
    let mvhd = find_box(&moov_kids, b"mvhd");
    assert_eq!(read_u32_be(mvhd.payload, mvhd.payload.len() - 4), 3);

    let traks: Vec<_> = moov_kids.iter().filter(|b| &b.fourcc == b"trak").collect();
    assert_eq!(traks.len(), 2);

    // Video trak inspection.
    let v_kids = parse_mp4_boxes(traks[0].payload);
    let v_mdia = parse_mp4_boxes(find_box(&v_kids, b"mdia").payload);
    let v_minf = parse_mp4_boxes(find_box(&v_mdia, b"minf").payload);
    let v_stbl = parse_mp4_boxes(find_box(&v_minf, b"stbl").payload);

    let v_stsz = find_box(&v_stbl, b"stsz").payload;
    let v_count = read_u32_be(v_stsz, 8) as usize;
    assert_eq!(v_count, 3);
    let v_sizes: Vec<u32> = (0..v_count)
        .map(|i| read_u32_be(v_stsz, 12 + i * 4))
        .collect();
    // AUD was stripped: sample 1 = 4+5(idr)=9, sample 2 = 4+4(p)=8, sample 3 = 4+5(idr)=9.
    assert_eq!(v_sizes, vec![9, 8, 9]);
    let v_sum: u32 = v_sizes.iter().sum();

    let v_stco = find_box(&v_stbl, b"stco").payload;
    let v_offset = read_u32_be(v_stco, 8) as usize;
    assert_eq!(v_offset, mdat.offset + 8);
    // Verify first sample in mdat starts with 4-byte length prefix + IDR NAL byte 0x65.
    assert_eq!(read_u32_be(&mp4, v_offset), 5);
    assert_eq!(mp4[v_offset + 4], 0x65);

    // Verify stss 1-based IDR indices = [1, 3].
    let v_stss = find_box(&v_stbl, b"stss").payload;
    assert_eq!(read_u32_be(v_stss, 4), 2);
    assert_eq!(read_u32_be(v_stss, 8), 1);
    assert_eq!(read_u32_be(v_stss, 12), 3);

    // Verify avcC carries exact SPS and PPS.
    let v_stsd = find_box(&v_stbl, b"stsd").payload;
    let avc1_boxes = parse_mp4_boxes(&v_stsd[8..]);
    let avc1 = find_box(&avc1_boxes, b"avc1");
    let avc1_children = parse_mp4_boxes(&avc1.payload[78..]);
    let avcc = find_box(&avc1_children, b"avcC").payload;
    let sps_len = u16::from_be_bytes([avcc[6], avcc[7]]) as usize;
    assert_eq!(&avcc[8..8 + sps_len], sps.as_slice());
    let pps_off = 8 + sps_len + 1;
    let pps_len = u16::from_be_bytes([avcc[pps_off], avcc[pps_off + 1]]) as usize;
    assert_eq!(&avcc[pps_off + 2..pps_off + 2 + pps_len], pps.as_slice());

    // Audio trak inspection.
    let a_kids = parse_mp4_boxes(traks[1].payload);
    let a_mdia = parse_mp4_boxes(find_box(&a_kids, b"mdia").payload);
    let a_minf = parse_mp4_boxes(find_box(&a_mdia, b"minf").payload);
    let a_stbl = parse_mp4_boxes(find_box(&a_minf, b"stbl").payload);

    let a_stsz = find_box(&a_stbl, b"stsz").payload;
    assert_eq!(read_u32_be(a_stsz, 8), 2);
    let a_sizes = [read_u32_be(a_stsz, 12), read_u32_be(a_stsz, 16)];
    assert_eq!(a_sizes, [4, 3]);
    let a_sum: u32 = a_sizes.iter().sum();
    assert_eq!(mdat.payload.len(), (v_sum + a_sum) as usize);

    let a_stco = find_box(&a_stbl, b"stco").payload;
    let a_offset = read_u32_be(a_stco, 8) as usize;
    assert_eq!(a_offset, v_offset + v_sum as usize);
    assert_eq!(&mp4[a_offset..a_offset + 4], &[0xDE, 0xAD, 0xBE, 0xEF]);
    assert_eq!(&mp4[a_offset + 4..a_offset + 7], &[0xCA, 0xFE, 0xBA]);

    // Verify esds carries exact AudioSpecificConfig [0x11, 0x90].
    let a_stsd = find_box(&a_stbl, b"stsd").payload;
    let mp4a_boxes = parse_mp4_boxes(&a_stsd[8..]);
    let mp4a = find_box(&mp4a_boxes, b"mp4a");
    let mp4a_children = parse_mp4_boxes(&mp4a.payload[28..]);
    let esds = find_box(&mp4a_children, b"esds").payload;
    assert!(
        esds.windows(4).any(|w| w == [0x05, 0x02, 0x11, 0x90]),
        "esds must embed DecoderSpecificInfo with AudioSpecificConfig"
    );
}

#[test]
fn mp4_mux_error_and_edge_cases() {
    let sps_nal = vec![0, 0, 0, 1, 0x67, 0x42, 0x00, 0x1F];
    let pps_nal = vec![0, 0, 0, 1, 0x68, 0xCE, 0x38, 0x80];
    let p_nal = vec![0, 0, 0, 1, 0x41, 0x9A, 0x24];

    // Empty frames -> error.
    assert!(mux_h264_mp4(64, 64, 12, &[]).is_err());

    // Missing SPS -> error.
    let mut no_sps = pps_nal.clone();
    no_sps.extend_from_slice(&p_nal);
    assert!(mux_h264_mp4(64, 64, 12, &[no_sps]).is_err());

    // Missing PPS -> error.
    let mut no_pps = sps_nal.clone();
    no_pps.extend_from_slice(&p_nal);
    assert!(mux_h264_mp4(64, 64, 12, &[no_pps]).is_err());

    // Zero slices (only SPS + PPS) -> error.
    let mut only_params = sps_nal.clone();
    only_params.extend_from_slice(&pps_nal);
    assert!(mux_h264_mp4(64, 64, 12, &[only_params]).is_err());

    // fps == 0 clamps to 1 and non-IDR stream falls back to stss = [1].
    let mut non_idr = sps_nal;
    non_idr.extend_from_slice(&pps_nal);
    non_idr.extend_from_slice(&p_nal);
    let mp4 = mux_h264_mp4(64, 64, 0, &[non_idr]).expect("fps=0 and non-IDR fallback");
    let top = parse_mp4_boxes(&mp4);
    let moov_kids = parse_mp4_boxes(top[2].payload);
    let trak_kids = parse_mp4_boxes(find_box(&moov_kids, b"trak").payload);
    let mdia_kids = parse_mp4_boxes(find_box(&trak_kids, b"mdia").payload);
    let minf_kids = parse_mp4_boxes(find_box(&mdia_kids, b"minf").payload);
    let stbl_kids = parse_mp4_boxes(find_box(&minf_kids, b"stbl").payload);
    let stts = find_box(&stbl_kids, b"stts").payload;
    assert_eq!(read_u32_be(stts, 12), 90_000, "fps=0 must clamp to 1 fps");
    let stss = find_box(&stbl_kids, b"stss").payload;
    assert_eq!(read_u32_be(stss, 4), 1);
    assert_eq!(read_u32_be(stss, 8), 1);
}

#[test]
fn split_annex_b_nals_handles_3byte_4byte_mixed_and_empty() {
    assert!(split_annex_b_nals(&[]).is_empty());
    assert!(split_annex_b_nals(&[1, 2, 3, 4]).is_empty());

    let data = [
        0, 0, 0, 1, 0x67, 0x01, // 4-byte start code
        0, 0, 1, 0x68, 0x02, 0x03, // 3-byte start code
        0, 0, 0, 1, // empty NAL skipped
        0, 0, 1, 0x65, // final 1-byte NAL
    ];
    let nals = split_annex_b_nals(&data);
    assert_eq!(
        nals,
        vec![&[0x67, 0x01][..], &[0x68, 0x02, 0x03][..], &[0x65][..]]
    );
}

fn build_adts_frame(
    profile_lc: u8,
    sr_idx: u8,
    ch_cfg: u8,
    with_crc: bool,
    payload: &[u8],
) -> Vec<u8> {
    let header_len = if with_crc { 9 } else { 7 };
    let frame_len = header_len + payload.len();
    let protection_absent: u8 = if with_crc { 0 } else { 1 };
    let mut out = vec![
        0xFF,
        0xF0 | protection_absent,
        ((profile_lc & 0x03) << 6) | ((sr_idx & 0x0F) << 2) | ((ch_cfg >> 2) & 0x01),
        ((ch_cfg & 0x03) << 6) | (((frame_len >> 11) & 0x03) as u8),
        ((frame_len >> 3) & 0xFF) as u8,
        (((frame_len & 0x07) << 5) as u8) | 0x1F,
        0xFC,
    ];
    if with_crc {
        out.extend_from_slice(&[0x12, 0x34]); // 2-byte CRC
    }
    out.extend_from_slice(payload);
    out
}

#[test]
fn parse_adts_frames_platform_independent_cases() {
    // 7-byte header (no CRC), 48 kHz (sr_idx=3), stereo (ch_cfg=2), with leading garbage.
    let mut stream = vec![0x00, 0x12, 0xFF, 0x00, 0xAB]; // garbage bytes before sync
    stream.extend_from_slice(&build_adts_frame(1, 3, 2, false, b"frameA"));
    stream.extend_from_slice(&build_adts_frame(1, 3, 2, false, b"frameBB"));
    let track = parse_adts_frames(&stream).expect("valid 7-byte ADTS");
    assert_eq!(track.sample_rate, 48_000);
    assert_eq!(track.channels, 2);
    // AAC-LC (2) | sr_idx=3 | ch=2 -> 00010_001 | 1_0010_000 = [0x11, 0x90]
    assert_eq!(track.audio_specific_config, [0x11, 0x90]);
    assert_eq!(track.frames, vec![b"frameA".to_vec(), b"frameBB".to_vec()]);

    // 9-byte CRC header (`protection_absent = 0`), 44.1 kHz (sr_idx=4), mono (ch_cfg=1).
    let crc_stream = build_adts_frame(1, 4, 1, true, b"crcpayload");
    let crc_track = parse_adts_frames(&crc_stream).expect("valid 9-byte CRC ADTS");
    assert_eq!(crc_track.sample_rate, 44_100);
    assert_eq!(crc_track.channels, 1);
    assert_eq!(crc_track.frames, vec![b"crcpayload".to_vec()]);

    // ch_cfg == 0 clamps channels to 1.
    let ch0_stream = build_adts_frame(1, 0, 0, false, b"hi");
    let ch0 = parse_adts_frames(&ch0_stream).expect("ch_cfg=0 clamps to 1");
    assert_eq!(ch0.sample_rate, 96_000);
    assert_eq!(ch0.channels, 1);

    // Out-of-range sample-rate index (15) -> None.
    let bad_sr = build_adts_frame(1, 15, 1, false, b"bad");
    assert!(parse_adts_frames(&bad_sr).is_none());

    // Truncated frame_len (claims more bytes than present) -> None.
    let mut trunc = build_adts_frame(1, 3, 1, false, b"abcdef");
    trunc.pop();
    assert!(parse_adts_frames(&trunc).is_none());

    // frame_len <= header_len (zero payload) -> None.
    let empty_payload = build_adts_frame(1, 3, 1, false, &[]);
    assert!(parse_adts_frames(&empty_payload).is_none());
}

#[test]
fn wav_waveform_and_finish_audio_clip_edge_cases() {
    // Stereo WAV header fields.
    let wav = encode_wav_pcm16(&[100, -100, 200, -200], 44_100, 2);
    assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 36 + 8);
    assert_eq!(u16::from_le_bytes(wav[20..22].try_into().unwrap()), 1); // PCM
    assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 2); // stereo
    assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 44_100);
    assert_eq!(
        u32::from_le_bytes(wav[28..32].try_into().unwrap()),
        44_100 * 4
    );
    assert_eq!(u16::from_le_bytes(wav[32..34].try_into().unwrap()), 4);
    assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16);
    assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 8);
    assert_eq!(i16::from_le_bytes([wav[44], wav[45]]), 100);
    assert_eq!(i16::from_le_bytes([wav[46], wav[47]]), -100);

    // downsample_waveform edge cases.
    assert!(downsample_waveform(&[1000, 2000], 0).is_empty());
    assert!(downsample_waveform(&[], 16).is_empty());
    let fewer = downsample_waveform(&[10_000, -10_000, 5_000], 8);
    assert_eq!(fewer.len(), 8);
    assert!(fewer.iter().all(|&b| (0.08..=1.0).contains(&b)));
    let zeros = downsample_waveform(&vec![0i16; 200], 10);
    assert_eq!(zeros.len(), 10);
    assert!(zeros.iter().all(|&b| (b - 0.08).abs() < 1e-6));

    // finish_audio_clip rejects empty PCM.
    assert!(matches!(
        finish_audio_clip(Vec::new(), 100),
        Err(HuddleError::Audio(_))
    ));
}

struct MockClipFrameSource {
    frames: VecDeque<Option<CapturedRgbFrame>>,
    stopped: Arc<AtomicBool>,
}

impl ClipFrameSource for MockClipFrameSource {
    fn next_frame(&mut self) -> Option<CapturedRgbFrame> {
        self.frames.pop_front().flatten()
    }
}

impl Drop for MockClipFrameSource {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
    }
}

fn solid_rgb_frame(width: u32, height: u32, r: u8, g: u8, b: u8) -> CapturedRgbFrame {
    let mut rgb = Vec::with_capacity((width * height * 3) as usize);
    for _ in 0..(width * height) {
        rgb.extend_from_slice(&[r, g, b]);
    }
    CapturedRgbFrame { width, height, rgb }
}

fn video_frame_to_clip_rgb(frame: &VideoFrame, max_w: u32, max_h: u32) -> Option<CapturedRgbFrame> {
    video_frame_to_clip_pair(frame, max_w, max_h).map(|c| c.rgb)
}

#[test]
fn video_frame_to_clip_rgb_converts_bgra_and_macos_surface_without_camera_hardware() {
    // Guards: tiny or truncated frames return None.
    assert!(video_frame_to_clip_rgb(&VideoFrame::from_bgra(1, 1, vec![0; 4]), 64, 64).is_none());
    assert!(video_frame_to_clip_rgb(&VideoFrame::from_bgra(4, 4, vec![0; 8]), 64, 64).is_none());

    // Portable BGRA -> RGB conversion preserves channel order ([B,G,R,A] -> [R,G,B]).
    let mut bgra = Vec::with_capacity(4 * 4 * 4);
    for _ in 0..16 {
        bgra.extend_from_slice(&[30, 20, 10, 255]);
    }
    let converted = video_frame_to_clip_rgb(&VideoFrame::from_bgra(4, 4, bgra), 64, 64)
        .expect("valid BGRA converts to RGB");
    assert_eq!((converted.width, converted.height), (4, 4));
    assert_eq!(&converted.rgb[0..3], &[10, 20, 30]);

    // On macOS, verify the exact MacVideoSurface -> VTPixelTransfer/vImage -> RGB clip path
    // using a VideoToolbox-decoded synthetic H.264 frame (zero camera hardware touched).
    #[cfg(target_os = "macos")]
    {
        let mut enc = H264VideoEncoder::new_camera().unwrap();
        let src = solid_rgb_frame(160, 96, 40, 180, 220);
        let bs = enc
            .encode_rgb(&src.rgb, src.width as usize, src.height as usize)
            .unwrap();
        let mut vt = crate::macos::VtH264Decoder::new().unwrap();
        let surface_vf = vt
            .decode_annex_b(&bs)
            .expect("VT should decode synthetic frame");
        assert!(surface_vf.mac_surface().is_some());

        let clip_rgb = video_frame_to_clip_rgb(&surface_vf, 80, 48)
            .expect("MacVideoSurface should convert and downscale for clip recording");
        assert_eq!((clip_rgb.width, clip_rgb.height), (80, 48));
        assert_eq!(clip_rgb.rgb.len(), 80 * 48 * 3);
    }
}

#[test]
fn video_clip_recorder_pipeline_captures_locks_dims_muxes_mp4_and_handles_errors() {
    // 1. Source startup failure propagates cleanly without leaking threads.
    let open_err = VideoClipRecorder::start_with_source_factory::<MockClipFrameSource, _>(
        || Err(HuddleError::Video("no camera device found".into())),
        CAMERA_MAX_W,
        CAMERA_MAX_H,
        false,
        Duration::from_millis(1),
    )
    .err()
    .expect("startup failure must return Err");
    assert!(open_err.to_string().contains("no camera device found"));

    // 2. End-to-end capture with None warm-up tick, valid frames, and mid-clip resize.
    let stopped = Arc::new(AtomicBool::new(false));
    let stopped_cb = Arc::clone(&stopped);
    let queued = VecDeque::from(vec![
        None, // camera warming up
        Some(solid_rgb_frame(64, 48, 220, 60, 40)),
        Some(solid_rgb_frame(64, 48, 40, 200, 80)),
        // Different dimensions mid-clip: must be resized/locked to initial 64x48.
        Some(solid_rgb_frame(96, 72, 60, 100, 240)),
        // Truncated frame must be ignored safely without crashing.
        Some(CapturedRgbFrame {
            width: 64,
            height: 48,
            rgb: vec![0; 10],
        }),
    ]);
    let rec = VideoClipRecorder::start_with_source_factory(
        move || {
            Ok(MockClipFrameSource {
                frames: queued,
                stopped: stopped_cb,
            })
        },
        CAMERA_MAX_W,
        CAMERA_MAX_H,
        false,
        Duration::from_millis(1),
    )
    .expect("mock recorder should start");

    // Wait until all 3 valid frames have been encoded (bounded wait so test is fast & deterministic).
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if rec.frames.lock().is_ok_and(|f| f.len() >= 3) {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(rec.latest_preview().is_some());

    let clip = rec.stop().expect("stop should mux valid MP4 clip");
    assert!(stopped.load(Ordering::Relaxed));
    assert_eq!(clip.mime, "video/mp4");
    assert!(clip.filename.ends_with(".mp4"));
    assert!(clip.preview_frame.is_some());
    assert!(!clip.thumbnail_frames.is_empty());

    let top = parse_mp4_boxes(&clip.bytes);
    assert_eq!(top.len(), 3);
    assert_eq!(&top[0].fourcc, b"ftyp");
    assert_eq!(&top[1].fourcc, b"mdat");
    assert_eq!(&top[2].fourcc, b"moov");

    // 3. Empty capture returns "no video frames captured" error on stop.
    let empty_stopped = Arc::new(AtomicBool::new(false));
    let empty_stopped_cb = Arc::clone(&empty_stopped);
    let empty_rec = VideoClipRecorder::start_with_source_factory(
        move || {
            Ok(MockClipFrameSource {
                frames: VecDeque::new(),
                stopped: empty_stopped_cb,
            })
        },
        CAMERA_MAX_W,
        CAMERA_MAX_H,
        false,
        Duration::from_millis(1),
    )
    .unwrap();
    let empty_err = empty_rec.stop().expect_err("zero frames must fail");
    assert!(empty_err.to_string().contains("no video frames captured"));
    assert!(empty_stopped.load(Ordering::Relaxed));

    // 4. Cancel stops the worker, which drops (stops) the source.
    let cancel_stopped = Arc::new(AtomicBool::new(false));
    let cancel_stopped_cb = Arc::clone(&cancel_stopped);
    let cancel_rec = VideoClipRecorder::start_with_source_factory(
        move || {
            Ok(MockClipFrameSource {
                frames: VecDeque::from(vec![Some(solid_rgb_frame(64, 48, 10, 20, 30))]),
                stopped: cancel_stopped_cb,
            })
        },
        CAMERA_MAX_W,
        CAMERA_MAX_H,
        false,
        Duration::from_millis(1),
    )
    .unwrap();
    cancel_rec.cancel();
    assert!(cancel_stopped.load(Ordering::Relaxed));
}

#[test]
fn letterbox_rgb_centers_smaller_frame_and_mp4_duration_matches_audio_without_drift() {
    // 1. Letterboxing a 2x2 white square into a 6x4 canvas centers it with black borders.
    let src = vec![255u8; 2 * 2 * 3];
    let padded = super::video::letterbox_rgb(&src, 2, 2, 6, 4);
    assert_eq!(padded.len(), 6 * 4 * 3);
    // Top-left corner (0,0) is black, interior (2,1) is white.
    assert_eq!(&padded[0..3], &[0, 0, 0]);
    let white_off = (6 + 2) * 3;
    assert_eq!(&padded[white_off..white_off + 3], &[255, 255, 255]);

    // 2. Duration-accurate MP4 muxing: 3500 frames over 300s (~11.67 fps) with matching 300s
    //    audio must align mvhd duration with the real 300s wall-clock duration instead of
    //    truncating to integer 11 fps or drifting A/V.
    let (_sps, _pps, base) = synthetic_h264_frames();
    let frames: Vec<Vec<u8>> = (0..35).map(|i| base[i % base.len()].clone()).collect();
    // 3s clip (3000 ms) with 35 video frames (~11.67 fps) + 141 AAC frames (~3.008s @ 48kHz).
    let audio = AacAudioTrack {
        sample_rate: 48_000,
        channels: 1,
        audio_specific_config: [0x11, 0x88],
        frames: vec![vec![0xAA, 0xBB]; 141],
    };
    let mp4 = super::mp4::mux_h264_aac_mp4_with_duration(320, 240, 3_000, &frames, Some(&audio))
        .expect("duration-based mux must succeed");
    let top = parse_mp4_boxes(&mp4);
    let moov_kids = parse_mp4_boxes(top[2].payload);
    let mvhd = find_box(&moov_kids, b"mvhd").payload;
    // mvhd v0: FullBox(4) + creation(4) + mod(4) + timescale(4) + duration(4).
    let mvhd_timescale = read_u32_be(mvhd, 12);
    let mvhd_duration = read_u32_be(mvhd, 16);
    assert_eq!(mvhd_timescale, 90_000);
    // Audio duration is 141 * 1024 / 48000 * 90000 = 270_720 ticks (~3.008s).
    // Video duration is 35 * round(270_000 / 35) = 35 * 7714 = 269_990 ticks (~2.9999s).
    // mvhd must take max(video, audio) so players never cut off trailing audio!
    assert_eq!(mvhd_duration, 270_720);
}
