//! Pure-Rust ISO-BMFF H.264 (+ optional AAC) `.mp4` muxer (`ftyp`/`mdat`/`moov`/`avcC`).

use crate::video::split_annex_b_nals;
use crate::{HuddleError, Result};

use super::audio::AacAudioTrack;

const UNITY_MATRIX: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];

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

/// Shared track specification for building either a video (`vide`) or audio (`soun`) ISO-BMFF `trak`.
struct TrakSpec<'a> {
    track_id: u32,
    movie_duration: u32,
    alternate_group: u16,
    volume: u16,
    width: u32,
    height: u32,
    media_timescale: u32,
    media_duration: u32,
    handler_type: &'a [u8; 4],
    handler_name: &'a [u8],
    media_header: Vec<u8>,
    sample_entry: Vec<u8>,
    sample_sizes: &'a [u32],
    sample_delta: u32,
    chunk_offset: u32,
    sync_samples: Option<&'a [u32]>,
}

fn build_mp4_trak(spec: &TrakSpec<'_>) -> Vec<u8> {
    let mut stsd_body = Vec::new();
    stsd_body.extend_from_slice(&1u32.to_be_bytes());
    stsd_body.extend_from_slice(&spec.sample_entry);
    let stsd = mp4_full_box(b"stsd", 0, 0, &stsd_body);

    let mut stts_body = Vec::new();
    stts_body.extend_from_slice(&1u32.to_be_bytes());
    stts_body.extend_from_slice(&(spec.sample_sizes.len() as u32).to_be_bytes());
    stts_body.extend_from_slice(&spec.sample_delta.to_be_bytes());
    let stts = mp4_full_box(b"stts", 0, 0, &stts_body);

    let stss = spec.sync_samples.map(|sync| {
        let mut stss_body = Vec::new();
        stss_body.extend_from_slice(&(sync.len() as u32).to_be_bytes());
        for &idx in sync {
            stss_body.extend_from_slice(&idx.to_be_bytes());
        }
        mp4_full_box(b"stss", 0, 0, &stss_body)
    });

    let mut stsc_body = Vec::new();
    stsc_body.extend_from_slice(&1u32.to_be_bytes()); // entry_count = 1 chunk holds all samples
    stsc_body.extend_from_slice(&1u32.to_be_bytes()); // first_chunk
    stsc_body.extend_from_slice(&(spec.sample_sizes.len() as u32).to_be_bytes());
    stsc_body.extend_from_slice(&1u32.to_be_bytes()); // sample_description_index
    let stsc = mp4_full_box(b"stsc", 0, 0, &stsc_body);

    let mut stsz_body = Vec::new();
    stsz_body.extend_from_slice(&0u32.to_be_bytes()); // variable sample sizes
    stsz_body.extend_from_slice(&(spec.sample_sizes.len() as u32).to_be_bytes());
    for &sz in spec.sample_sizes {
        stsz_body.extend_from_slice(&sz.to_be_bytes());
    }
    let stsz = mp4_full_box(b"stsz", 0, 0, &stsz_body);

    let mut stco_body = Vec::new();
    stco_body.extend_from_slice(&1u32.to_be_bytes());
    stco_body.extend_from_slice(&spec.chunk_offset.to_be_bytes());
    let stco = mp4_full_box(b"stco", 0, 0, &stco_body);

    let mut stbl_body = Vec::new();
    stbl_body.extend_from_slice(&stsd);
    stbl_body.extend_from_slice(&stts);
    if let Some(stss_box) = &stss {
        stbl_body.extend_from_slice(stss_box);
    }
    stbl_body.extend_from_slice(&stsc);
    stbl_body.extend_from_slice(&stsz);
    stbl_body.extend_from_slice(&stco);
    let stbl = mp4_box(b"stbl", &stbl_body);

    let url_box = mp4_full_box(b"url ", 0, 1, &[]);
    let mut dref_body = Vec::new();
    dref_body.extend_from_slice(&1u32.to_be_bytes());
    dref_body.extend_from_slice(&url_box);
    let dref = mp4_full_box(b"dref", 0, 0, &dref_body);
    let dinf = mp4_box(b"dinf", &dref);

    let mut minf_body = Vec::new();
    minf_body.extend_from_slice(&spec.media_header);
    minf_body.extend_from_slice(&dinf);
    minf_body.extend_from_slice(&stbl);
    let minf = mp4_box(b"minf", &minf_body);

    let mut mdhd_body = Vec::new();
    mdhd_body.extend_from_slice(&0u32.to_be_bytes()); // creation_time
    mdhd_body.extend_from_slice(&0u32.to_be_bytes()); // modification_time
    mdhd_body.extend_from_slice(&spec.media_timescale.to_be_bytes());
    mdhd_body.extend_from_slice(&spec.media_duration.to_be_bytes());
    mdhd_body.extend_from_slice(&0x55C4u16.to_be_bytes()); // und language
    mdhd_body.extend_from_slice(&0u16.to_be_bytes());
    let mdhd = mp4_full_box(b"mdhd", 0, 0, &mdhd_body);

    let mut hdlr_body = Vec::new();
    hdlr_body.extend_from_slice(&0u32.to_be_bytes()); // pre_defined
    hdlr_body.extend_from_slice(spec.handler_type);
    hdlr_body.extend_from_slice(&[0u8; 12]); // reserved
    hdlr_body.extend_from_slice(spec.handler_name);
    let hdlr = mp4_full_box(b"hdlr", 0, 0, &hdlr_body);

    let mut mdia_body = Vec::new();
    mdia_body.extend_from_slice(&mdhd);
    mdia_body.extend_from_slice(&hdlr);
    mdia_body.extend_from_slice(&minf);
    let mdia = mp4_box(b"mdia", &mdia_body);

    let mut tkhd_body = Vec::new();
    tkhd_body.extend_from_slice(&0u32.to_be_bytes()); // creation_time
    tkhd_body.extend_from_slice(&0u32.to_be_bytes()); // modification_time
    tkhd_body.extend_from_slice(&spec.track_id.to_be_bytes());
    tkhd_body.extend_from_slice(&0u32.to_be_bytes()); // reserved
    tkhd_body.extend_from_slice(&spec.movie_duration.to_be_bytes());
    tkhd_body.extend_from_slice(&[0u8; 8]); // reserved
    tkhd_body.extend_from_slice(&0u16.to_be_bytes()); // layer
    tkhd_body.extend_from_slice(&spec.alternate_group.to_be_bytes());
    tkhd_body.extend_from_slice(&spec.volume.to_be_bytes());
    tkhd_body.extend_from_slice(&0u16.to_be_bytes()); // reserved
    for m in UNITY_MATRIX {
        tkhd_body.extend_from_slice(&m.to_be_bytes());
    }
    tkhd_body.extend_from_slice(&(spec.width << 16).to_be_bytes());
    tkhd_body.extend_from_slice(&(spec.height << 16).to_be_bytes());
    let tkhd = mp4_full_box(b"tkhd", 0, 0x0000_0003, &tkhd_body); // enabled + in_movie

    let mut trak_body = Vec::new();
    trak_body.extend_from_slice(&tkhd);
    trak_body.extend_from_slice(&mdia);
    mp4_box(b"trak", &trak_body)
}

/// Mux Annex-B H.264 access units into a self-contained ISO-BMFF `.mp4` container.
pub fn mux_h264_mp4(width: u32, height: u32, fps: u32, frames: &[Vec<u8>]) -> Result<Vec<u8>> {
    mux_h264_aac_mp4(width, height, fps, frames, None)
}

/// Mux Annex-B H.264 video frames + optional raw AAC-LC audio track into an ISO-BMFF `.mp4`.
/// Mux Annex-B H.264 video frames (and optional AAC audio) into an ISO BMFF MP4 container using
/// the exact recorded duration in milliseconds so fractional frame rates never drift A/V sync.
pub(crate) fn mux_h264_aac_mp4_with_duration(
    width: u32,
    height: u32,
    elapsed_ms: u64,
    frames: &[Vec<u8>],
    audio: Option<&AacAudioTrack>,
) -> Result<Vec<u8>> {
    let timescale: u32 = 90_000;
    let count = frames.len().max(1) as u64;
    // Exact ticks per frame at 90 kHz: 30 fps = 3_000 ticks, 4 fps = 22_500 ticks.
    let sample_delta: u32 =
        ((elapsed_ms.max(250).saturating_mul(90)) / count).clamp(3_000, 22_500) as u32;
    mux_h264_aac_mp4_inner(width, height, timescale, sample_delta, frames, audio)
}

/// Mux Annex-B H.264 video frames (and optional AAC audio) into an ISO BMFF MP4 container.
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
    mux_h264_aac_mp4_inner(width, height, timescale, sample_delta, frames, audio)
}

fn mux_h264_aac_mp4_inner(
    width: u32,
    height: u32,
    timescale: u32,
    sample_delta: u32,
    frames: &[Vec<u8>],
    audio: Option<&AacAudioTrack>,
) -> Result<Vec<u8>> {
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
    let total_duration = (sample_sizes.len() as u32).saturating_mul(sample_delta);
    let audio_duration_90k = audio
        .filter(|_| !audio_sizes.is_empty())
        .map(|a| {
            let sr = u64::from(a.sample_rate.max(8_000));
            let samples = (audio_sizes.len() as u64).saturating_mul(1024);
            ((samples.saturating_mul(u64::from(timescale))) / sr).min(u64::from(u32::MAX)) as u32
        })
        .unwrap_or(0);
    let movie_duration = total_duration.max(audio_duration_90k);

    let video_trak = build_video_mp4_trak(&VideoTrakParams {
        width,
        height,
        sps: &sps,
        pps: &pps,
        sample_sizes: &sample_sizes,
        sync_samples: &sync_samples,
        timescale,
        sample_delta,
        total_duration,
        chunk_offset: video_chunk_offset,
    });

    let has_audio = audio.is_some() && !audio_sizes.is_empty();
    let next_track_id: u32 = if has_audio { 3 } else { 2 };
    let mut mvhd_body = Vec::new();
    mvhd_body.extend_from_slice(&0u32.to_be_bytes());
    mvhd_body.extend_from_slice(&0u32.to_be_bytes());
    mvhd_body.extend_from_slice(&timescale.to_be_bytes());
    mvhd_body.extend_from_slice(&movie_duration.to_be_bytes());
    mvhd_body.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // rate 1.0
    mvhd_body.extend_from_slice(&0x0100u16.to_be_bytes()); // volume 1.0
    mvhd_body.extend_from_slice(&[0u8; 10]); // reserved
    for m in UNITY_MATRIX {
        mvhd_body.extend_from_slice(&m.to_be_bytes());
    }
    mvhd_body.extend_from_slice(&[0u8; 24]); // pre_defined
    mvhd_body.extend_from_slice(&next_track_id.to_be_bytes());
    let mvhd = mp4_full_box(b"mvhd", 0, 0, &mvhd_body);

    let mut moov_body = Vec::new();
    moov_body.extend_from_slice(&mvhd);
    moov_body.extend_from_slice(&video_trak);
    if has_audio && let Some(a) = audio {
        let audio_trak = build_aac_mp4_trak(a, &audio_sizes, audio_chunk_offset, movie_duration);
        moov_body.extend_from_slice(&audio_trak);
    }
    let moov = mp4_box(b"moov", &moov_body);

    let mut out = Vec::with_capacity(ftyp.len() + mdat.len() + moov.len());
    out.extend_from_slice(&ftyp);
    out.extend_from_slice(&mdat);
    out.extend_from_slice(&moov);
    Ok(out)
}

struct VideoTrakParams<'a> {
    width: u32,
    height: u32,
    sps: &'a [u8],
    pps: &'a [u8],
    sample_sizes: &'a [u32],
    sync_samples: &'a [u32],
    timescale: u32,
    sample_delta: u32,
    total_duration: u32,
    chunk_offset: u32,
}

fn build_video_mp4_trak(p: &VideoTrakParams<'_>) -> Vec<u8> {
    let profile_idc = *p.sps.get(1).unwrap_or(&66);
    let profile_compat = *p.sps.get(2).unwrap_or(&0);
    let level_idc = *p.sps.get(3).unwrap_or(&31);
    let mut avcc_body = vec![1, profile_idc, profile_compat, level_idc, 0xFF, 0xE1];
    avcc_body.extend_from_slice(&(p.sps.len() as u16).to_be_bytes());
    avcc_body.extend_from_slice(p.sps);
    avcc_body.push(1);
    avcc_body.extend_from_slice(&(p.pps.len() as u16).to_be_bytes());
    avcc_body.extend_from_slice(p.pps);
    let avcc = mp4_box(b"avcC", &avcc_body);

    let mut avc1_body = Vec::new();
    avc1_body.extend_from_slice(&[0u8; 6]);
    avc1_body.extend_from_slice(&1u16.to_be_bytes());
    avc1_body.extend_from_slice(&[0u8; 16]);
    avc1_body.extend_from_slice(&(p.width as u16).to_be_bytes());
    avc1_body.extend_from_slice(&(p.height as u16).to_be_bytes());
    avc1_body.extend_from_slice(&0x0048_0000u32.to_be_bytes());
    avc1_body.extend_from_slice(&0x0048_0000u32.to_be_bytes());
    avc1_body.extend_from_slice(&0u32.to_be_bytes());
    avc1_body.extend_from_slice(&1u16.to_be_bytes());
    avc1_body.extend_from_slice(&[0u8; 32]);
    avc1_body.extend_from_slice(&0x0018u16.to_be_bytes());
    avc1_body.extend_from_slice(&(-1i16).to_be_bytes());
    avc1_body.extend_from_slice(&avcc);

    build_mp4_trak(&TrakSpec {
        track_id: 1,
        movie_duration: p.total_duration,
        alternate_group: 0,
        volume: 0,
        width: p.width,
        height: p.height,
        media_timescale: p.timescale,
        media_duration: p.total_duration,
        handler_type: b"vide",
        handler_name: b"VideoHandler\0",
        media_header: mp4_full_box(b"vmhd", 0, 1, &[0u8; 8]),
        sample_entry: mp4_box(b"avc1", &avc1_body),
        sample_sizes: p.sample_sizes,
        sample_delta: p.sample_delta,
        chunk_offset: p.chunk_offset,
        sync_samples: Some(p.sync_samples),
    })
}

fn build_aac_mp4_trak(
    audio: &AacAudioTrack,
    sample_sizes: &[u32],
    chunk_offset: u32,
    movie_duration: u32,
) -> Vec<u8> {
    let audio_timescale = audio.sample_rate.max(8_000);
    let sample_delta = 1024u32;
    let audio_duration = sample_sizes.len() as u32 * sample_delta;

    let asc = audio.audio_specific_config;
    let dec_spec = vec![0x05, 0x02, asc[0], asc[1]];
    let mut dec_cfg = vec![
        0x04,
        (13 + dec_spec.len()) as u8,
        0x40,
        0x15,
        0x00,
        0x00,
        0x00,
    ];
    dec_cfg.extend_from_slice(&128_000u32.to_be_bytes());
    dec_cfg.extend_from_slice(&96_000u32.to_be_bytes());
    dec_cfg.extend_from_slice(&dec_spec);
    let sl_cfg = [0x06u8, 0x01, 0x02];
    let mut es_desc = vec![
        0x03,
        (3 + dec_cfg.len() + sl_cfg.len()) as u8,
        0x00,
        0x02,
        0x00,
    ];
    es_desc.extend_from_slice(&dec_cfg);
    es_desc.extend_from_slice(&sl_cfg);
    let esds = mp4_full_box(b"esds", 0, 0, &es_desc);

    let mut mp4a_body = Vec::new();
    mp4a_body.extend_from_slice(&[0u8; 6]);
    mp4a_body.extend_from_slice(&1u16.to_be_bytes());
    mp4a_body.extend_from_slice(&[0u8; 8]);
    mp4a_body.extend_from_slice(&audio.channels.to_be_bytes());
    mp4a_body.extend_from_slice(&16u16.to_be_bytes());
    mp4a_body.extend_from_slice(&0u32.to_be_bytes());
    mp4a_body.extend_from_slice(&(audio_timescale << 16).to_be_bytes());
    mp4a_body.extend_from_slice(&esds);

    build_mp4_trak(&TrakSpec {
        track_id: 2,
        movie_duration,
        alternate_group: 1,
        volume: 0x0100,
        width: 0,
        height: 0,
        media_timescale: audio_timescale,
        media_duration: audio_duration,
        handler_type: b"soun",
        handler_name: b"SoundHandler\0",
        media_header: mp4_full_box(b"smhd", 0, 0, &[0u8; 4]),
        sample_entry: mp4_box(b"mp4a", &mp4a_body),
        sample_sizes,
        sample_delta,
        chunk_offset,
        sync_samples: None,
    })
}
