//! Pure-Rust ISO-BMFF H.264 (+ optional AAC) `.mp4` muxer (`ftyp`/`mdat`/`moov`/`avcC`).

use crate::{HuddleError, Result};

use super::audio::AacAudioTrack;

// ---------------------------------------------------------------------------
// Pure-Rust ISO-BMFF H.264 `.mp4` muxer (`ftyp` + `mdat` + `moov`/`avcC`)
// ---------------------------------------------------------------------------

pub(super) fn split_annex_b_nals(data: &[u8]) -> Vec<&[u8]> {
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

    let has_audio = audio.is_some() && !audio_sizes.is_empty();
    let next_track_id: u32 = if has_audio { 3 } else { 2 };
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
    if has_audio && let Some(a) = audio {
        let audio_trak = build_aac_mp4_trak(
            a,
            &audio_sizes,
            audio_chunk_offset,
            total_duration,
            unity_matrix,
        );
        moov_body.extend_from_slice(&audio_trak);
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
