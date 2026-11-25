//! Pure-Rust AV1 RTP depacketizer and `rav1d` AV1 video decoder for incoming Slack Huddle
//! screen shares and video streams (official Slack clients publish `#content` screen shares in AV1).

use std::ptr::NonNull;

use rav1d::include::dav1d::common::Dav1dDataProps;
use rav1d::include::dav1d::data::Dav1dData;
use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
use rav1d::include::dav1d::headers::{
    DAV1D_PIXEL_LAYOUT_I400, DAV1D_PIXEL_LAYOUT_I420, DAV1D_PIXEL_LAYOUT_I422,
    DAV1D_PIXEL_LAYOUT_I444,
};
use rav1d::include::dav1d::picture::Dav1dPicture;
use rav1d::src::lib::{
    dav1d_close, dav1d_data_create, dav1d_data_unref, dav1d_default_settings, dav1d_get_picture,
    dav1d_open, dav1d_picture_unref, dav1d_send_data,
};

use crate::video::VideoFrame;
use crate::{HuddleError, Result};

/// Reads an unsigned LEB128 integer; returns `(value, bytes_consumed)`.
fn read_leb128(buf: &[u8]) -> Option<(usize, usize)> {
    let mut value = 0usize;
    for (i, &byte) in buf.iter().take(8).enumerate() {
        value |= ((byte & 0x7f) as usize) << (i * 7);
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

/// Writes an unsigned LEB128 integer into `out`.
fn write_leb128(out: &mut Vec<u8>, mut value: usize) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            return;
        }
    }
}

/// Appends one wire OBU to a low-overhead bitstream temporal unit, restoring the
/// `obu_has_size_field` bit and LEB128 payload size expected by `rav1d` when stripped on the wire.
fn append_sized_obu(out: &mut Vec<u8>, obu: &[u8]) {
    let Some(&first) = obu.first() else {
        return;
    };
    // If the wire OBU already carries `obu_has_size_field` (bit 1), preserve it verbatim instead
    // of prepending a duplicate LEB128 size field.
    if first & 0b0000_0010 != 0 {
        out.extend_from_slice(obu);
        return;
    }
    let header_len = if first & 0b0000_0100 != 0 { 2 } else { 1 };
    if obu.len() < header_len {
        return;
    }
    out.push(first | 0b0000_0010);
    out.extend_from_slice(&obu[1..header_len]);
    write_leb128(out, obu.len() - header_len);
    out.extend_from_slice(&obu[header_len..]);
}

/// Reassembles RFC AV1 RTP packets into complete low-overhead AV1 temporal units.
#[derive(Default)]
pub struct Av1Frames {
    unit: Vec<u8>,
    partial: Vec<u8>,
    last_seq: Option<u16>,
    broken: bool,
    seq_gaps: u64,
    needs_keyframe: bool,
}

impl Av1Frames {
    /// Total number of RTP 16-bit sequence gaps observed on this stream.
    pub fn seq_gaps(&self) -> u64 {
        self.seq_gaps
    }

    /// Returns `true` (and clears the flag) if a mid-frame sequence gap requires a PLI keyframe.
    pub fn take_needs_keyframe(&mut self) -> bool {
        std::mem::take(&mut self.needs_keyframe)
    }

    /// Push one RTP packet payload; returns `Some(temporal_unit)` when `marker` closes a valid unit.
    pub fn push(&mut self, seq: u16, marker: bool, payload: &[u8]) -> Option<Vec<u8>> {
        let Some(&header) = payload.first() else {
            self.last_seq = Some(seq);
            return None;
        };
        let z_continues_prev = header & 0b1000_0000 != 0;
        let n_new_cvs = header & 0b0000_1000 != 0;

        let missing_pkts = match self.last_seq {
            None => {
                self.last_seq = Some(seq);
                0
            }
            Some(prev) => {
                let diff = seq.wrapping_sub(prev) as i16;
                if diff <= 0 {
                    // Late/reordered or duplicate RTP packet; drop without inflating loss or
                    // regressing `last_seq`.
                    return None;
                }
                self.last_seq = Some(seq);
                (diff - 1) as u64
            }
        };
        let seq_gap = missing_pkts > 0;
        if seq_gap {
            self.seq_gaps = self.seq_gaps.saturating_add(missing_pkts);
        }

        // An SFU routinely skips RTP sequence numbers between complete frames (e.g. dropped
        // non-reference frames, layer switches, or padding). Only treat a gap as breaking an
        // in-progress frame or a continuation packet (`Z=1`); a clean new frame (`Z=0`) when no
        // prior frame is in progress—or any New Coded Video Sequence (`N=1`, keyframe)—recovers
        // immediately instead of discarding the new frame.
        if n_new_cvs && !z_continues_prev {
            self.reset();
        } else if seq_gap {
            if !self.unit.is_empty() || !self.partial.is_empty() || z_continues_prev {
                self.broken = true;
                self.needs_keyframe = true;
            } else {
                self.reset();
            }
        }

        if self.broken {
            if marker {
                self.reset();
            }
            return None;
        }

        if self.parse_packet(payload).is_none() {
            self.broken = true;
            if marker {
                self.reset();
            }
            return None;
        }

        if !marker {
            return None;
        }
        if !self.partial.is_empty() {
            self.reset();
            return None;
        }
        let unit = std::mem::take(&mut self.unit);
        self.reset();
        (!unit.is_empty()).then_some(unit)
    }

    pub fn reset(&mut self) {
        self.unit.clear();
        self.partial.clear();
        self.broken = false;
    }

    fn parse_packet(&mut self, payload: &[u8]) -> Option<()> {
        let (&header, mut rest) = payload.split_first()?;
        let z_continues_prev = header & 0b1000_0000 != 0;
        let y_continues_next = header & 0b0100_0000 != 0;
        let w_count = (header & 0b0011_0000) >> 4;

        if !z_continues_prev && !self.partial.is_empty() {
            return None;
        }

        let mut idx = 0u8;
        while !rest.is_empty() {
            idx += 1;
            let (elem, next_rest) = if w_count != 0 && idx == w_count {
                (rest, &[][..])
            } else {
                let (len, n) = read_leb128(rest)?;
                let body = rest.get(n..)?;
                if len > body.len() {
                    return None;
                }
                (&body[..len], &body[len..])
            };
            rest = next_rest;

            let is_first = idx == 1;
            let is_last = rest.is_empty();

            if is_first && z_continues_prev {
                self.partial.extend_from_slice(elem);
            } else {
                self.flush_partial();
                self.partial.extend_from_slice(elem);
            }

            if !(is_last && y_continues_next) {
                self.flush_partial();
            }
        }
        Some(())
    }

    fn flush_partial(&mut self) {
        if self.partial.is_empty() {
            return;
        }
        let obu = std::mem::take(&mut self.partial);
        append_sized_obu(&mut self.unit, &obu);
    }
}

/// Pure-Rust AV1 decoder backed by `rav1d`, converting decoded YUV420 8-bit frames directly into
/// BGRA8888 [`VideoFrame`]s ready for GPUI `RenderImage`.
pub struct Av1Decoder {
    ctx: Option<Dav1dContext>,
}

// SAFETY: `Av1Decoder` owns its `Dav1dContext` exclusively and only accesses it through `&mut self`.
unsafe impl Send for Av1Decoder {}

impl Av1Decoder {
    pub fn new() -> Result<Self> {
        let mut settings = std::mem::MaybeUninit::<Dav1dSettings>::zeroed();
        let mut ctx: Option<Dav1dContext> = None;
        // SAFETY: `settings` and `ctx` are valid stack pointers matching dav1d's C ABI.
        let res = unsafe {
            dav1d_default_settings(NonNull::new_unchecked(settings.as_mut_ptr()));
            let mut settings = settings.assume_init();
            settings.n_threads = 2;
            settings.max_frame_delay = 1;
            dav1d_open(
                NonNull::new(&mut ctx as *mut _),
                NonNull::new(&mut settings as *mut _),
            )
        };
        if res.0 < 0 || ctx.is_none() {
            return Err(HuddleError::Codec(format!(
                "rav1d init failed (code {})",
                res.0
            )));
        }
        Ok(Self { ctx })
    }

    /// Decode one AV1 low-overhead temporal unit into a BGRA8888 [`VideoFrame`].
    pub fn decode(&mut self, unit: &[u8]) -> Result<Option<VideoFrame>> {
        if unit.is_empty() {
            return Ok(None);
        }
        // Prepend a 2-byte Temporal Delimiter OBU (`obu_type=2`, `obu_has_size_field=1`, `size=0`)
        // when stripped on the RTP wire so `rav1d` flushes the decoded picture immediately.
        let starts_with_td = ((unit[0] >> 3) & 0x0f) == 2;
        let total_len = unit.len() + if starts_with_td { 0 } else { 2 };

        // SAFETY: `Dav1dData` and `Dav1dPicture` are initialized per dav1d's FFI contract and
        // released with `dav1d_picture_unref` before returning.
        unsafe {
            let mut data = Dav1dData {
                data: None,
                sz: 0,
                r#ref: None,
                m: Dav1dDataProps::default(),
            };
            let buf = dav1d_data_create(NonNull::new(&mut data as *mut _), total_len);
            if buf.is_null() {
                return Err(HuddleError::Codec("rav1d buffer allocation failed".into()));
            }
            if starts_with_td {
                std::ptr::copy_nonoverlapping(unit.as_ptr(), buf, unit.len());
            } else {
                *buf = 0x12;
                *buf.add(1) = 0x00;
                std::ptr::copy_nonoverlapping(unit.as_ptr(), buf.add(2), unit.len());
            }

            let mut out_frame = None;
            let mut send_res = dav1d_send_data(self.ctx, NonNull::new(&mut data as *mut _));
            if send_res.0 == -11 {
                // EAGAIN: drain any queued picture first, then retry sending the new unit.
                if let Some(frame) = self.drain_picture()? {
                    out_frame = Some(frame);
                }
                send_res = dav1d_send_data(self.ctx, NonNull::new(&mut data as *mut _));
            }
            if send_res.0 < 0 && send_res.0 != -11 {
                dav1d_data_unref(NonNull::new(&mut data as *mut _));
                return Err(HuddleError::Codec(format!(
                    "rav1d send_data error {}",
                    send_res.0
                )));
            }
            if send_res.0 == -11 {
                dav1d_data_unref(NonNull::new(&mut data as *mut _));
            }

            if let Some(frame) = self.drain_picture()? {
                out_frame = Some(frame);
            }
            Ok(out_frame)
        }
    }

    unsafe fn drain_picture(&mut self) -> Result<Option<VideoFrame>> {
        unsafe {
            let mut pic = std::mem::MaybeUninit::<Dav1dPicture>::zeroed().assume_init();
            let res = dav1d_get_picture(self.ctx, NonNull::new(&mut pic as *mut _));
            if res.0 == -11 {
                return Ok(None);
            }
            if res.0 < 0 {
                return Err(HuddleError::Codec(format!(
                    "rav1d get_picture error {}",
                    res.0
                )));
            }
            let frame = picture_to_bgra(&pic);
            dav1d_picture_unref(NonNull::new(&mut pic as *mut _));
            Ok(frame)
        }
    }
}

impl Drop for Av1Decoder {
    fn drop(&mut self) {
        // SAFETY: closes the `Dav1dContext` opened in `Av1Decoder::new`.
        unsafe {
            dav1d_close(NonNull::new(&mut self.ctx as *mut _));
        }
    }
}

/// Fast integer fixed-point conversion of an 8-bit or 10-bit `Dav1dPicture` (`I420`, `I422`,
/// `I444`, or `I400`) into a BGRA8888 [`VideoFrame`] for GPUI.
unsafe fn picture_to_bgra(pic: &Dav1dPicture) -> Option<VideoFrame> {
    let w = pic.p.w.max(0) as usize;
    let h = pic.p.h.max(0) as usize;
    let bpc = pic.p.bpc;
    if w == 0 || h == 0 || (bpc != 8 && bpc != 10) {
        return None;
    }
    let layout = pic.p.layout;
    let (cw_shift, ch_shift, has_chroma) = match layout {
        DAV1D_PIXEL_LAYOUT_I420 => (1usize, 1usize, true),
        DAV1D_PIXEL_LAYOUT_I422 => (1usize, 0usize, true),
        DAV1D_PIXEL_LAYOUT_I444 => (0usize, 0usize, true),
        DAV1D_PIXEL_LAYOUT_I400 => (0usize, 0usize, false),
        _ => return None,
    };

    let y_ptr = pic.data[0]?.as_ptr() as *const u8;
    let u_ptr = if has_chroma {
        Some(pic.data[1]?.as_ptr() as *const u8)
    } else {
        None
    };
    let v_ptr = if has_chroma {
        Some(pic.data[2]?.as_ptr() as *const u8)
    } else {
        None
    };
    let y_stride = pic.stride[0];
    let c_stride = pic.stride[1];
    let mut bgra = vec![0u8; w * h * 4];

    #[inline(always)]
    fn write_yuv_pixel(dst: &mut [u8], y: i32, u: i32, v: i32) {
        let c = y - 16;
        let d = u - 128;
        let e = v - 128;
        let yy = 298 * c + 128;
        let r = (yy + 409 * e) >> 8;
        let g = (yy - 100 * d - 208 * e) >> 8;
        let b = (yy + 516 * d) >> 8;
        dst[0] = b.clamp(0, 255) as u8;
        dst[1] = g.clamp(0, 255) as u8;
        dst[2] = r.clamp(0, 255) as u8;
        dst[3] = 255;
    }

    // Hot path: 8-bit 4:2:0 processed in 2x2 blocks sharing chroma loads (zero float math).
    if bpc == 8
        && layout == DAV1D_PIXEL_LAYOUT_I420
        && let (Some(up), Some(vp)) = (u_ptr, v_ptr)
    {
        let pairs_h = h & !1;
        let pairs_w = w & !1;
        for row in (0..pairs_h).step_by(2) {
            let y_row0 = (row as isize) * y_stride;
            let y_row1 = ((row + 1) as isize) * y_stride;
            let c_row = ((row >> 1) as isize) * c_stride;
            let out_row0 = row * w * 4;
            let out_row1 = (row + 1) * w * 4;
            for col in (0..pairs_w).step_by(2) {
                let cx = (col >> 1) as isize;
                let u = unsafe { *up.offset(c_row + cx) } as i32;
                let v = unsafe { *vp.offset(c_row + cx) } as i32;
                let d = u - 128;
                let e = v - 128;
                let r_add = 409 * e + 128;
                let g_add = -100 * d - 208 * e + 128;
                let b_add = 516 * d + 128;

                let c00 = (unsafe { *y_ptr.offset(y_row0 + col as isize) } as i32 - 16) * 298;
                let c01 = (unsafe { *y_ptr.offset(y_row0 + (col + 1) as isize) } as i32 - 16) * 298;
                let c10 = (unsafe { *y_ptr.offset(y_row1 + col as isize) } as i32 - 16) * 298;
                let c11 = (unsafe { *y_ptr.offset(y_row1 + (col + 1) as isize) } as i32 - 16) * 298;

                let o00 = out_row0 + col * 4;
                let o01 = o00 + 4;
                let o10 = out_row1 + col * 4;
                let o11 = o10 + 4;

                for (o, cy) in [(o00, c00), (o01, c01), (o10, c10), (o11, c11)] {
                    bgra[o] = ((cy + b_add) >> 8).clamp(0, 255) as u8;
                    bgra[o + 1] = ((cy + g_add) >> 8).clamp(0, 255) as u8;
                    bgra[o + 2] = ((cy + r_add) >> 8).clamp(0, 255) as u8;
                    bgra[o + 3] = 255;
                }
            }
        }
        if pairs_h == h && pairs_w == w {
            return Some(VideoFrame::from_bgra(w as u32, h as u32, bgra));
        }
    }

    let shift16 = (bpc - 8) as u32;
    for row in 0..h {
        let y_row = (row as isize) * y_stride;
        let c_row = ((row >> ch_shift) as isize) * c_stride;
        let out_row = row * w * 4;
        for col in 0..w {
            let (y, u, v) = if bpc == 8 {
                let y = unsafe { *y_ptr.offset(y_row + col as isize) } as i32;
                let (u, v) = match (u_ptr, v_ptr) {
                    (Some(up), Some(vp)) => {
                        let cx = (col >> cw_shift) as isize;
                        (
                            unsafe { *up.offset(c_row + cx) } as i32,
                            unsafe { *vp.offset(c_row + cx) } as i32,
                        )
                    }
                    _ => (128, 128),
                };
                (y, u, v)
            } else {
                let y16 = unsafe { *(y_ptr.offset(y_row) as *const u16).add(col) };
                let y = (y16 >> shift16) as i32;
                let (u, v) = match (u_ptr, v_ptr) {
                    (Some(up), Some(vp)) => {
                        let cx = col >> cw_shift;
                        let u16v = unsafe { *(up.offset(c_row) as *const u16).add(cx) };
                        let v16v = unsafe { *(vp.offset(c_row) as *const u16).add(cx) };
                        ((u16v >> shift16) as i32, (v16v >> shift16) as i32)
                    }
                    _ => (128, 128),
                };
                (y, u, v)
            };
            let o = out_row + col * 4;
            write_yuv_pixel(&mut bgra[o..o + 4], y, u, v);
        }
    }
    Some(VideoFrame::from_bgra(w as u32, h as u32, bgra))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_obu(obu_type: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![obu_type << 3];
        v.extend_from_slice(payload);
        v
    }

    fn make_rtp_packet(z: bool, y: bool, elems: &[&[u8]]) -> Vec<u8> {
        let mut h = 0u8;
        if z {
            h |= 0b1000_0000;
        }
        if y {
            h |= 0b0100_0000;
        }
        let mut out = vec![h];
        for e in elems {
            write_leb128(&mut out, e.len());
            out.extend_from_slice(e);
        }
        out
    }

    #[test]
    fn av1_frames_reassembles_split_obus_and_restores_size_field() {
        let mut f = Av1Frames::default();
        let mut obu = make_obu(6, b"hello");
        let tail = obu.split_off(3);
        assert!(
            f.push(10, false, &make_rtp_packet(false, true, &[&obu]))
                .is_none()
        );
        let unit = f
            .push(11, true, &make_rtp_packet(true, false, &[&tail]))
            .expect("temporal unit");
        assert_eq!(unit, vec![(6 << 3) | 0b10, 5, b'h', b'e', b'l', b'l', b'o']);
    }

    #[test]
    fn av1_frames_recovers_after_sequence_gap_between_complete_frames() {
        let mut f = Av1Frames::default();
        let obu1 = make_obu(6, b"one");
        let u1 = f
            .push(10, true, &make_rtp_packet(false, false, &[&obu1]))
            .expect("first unit");
        assert_eq!(u1, vec![(6 << 3) | 0b10, 3, b'o', b'n', b'e']);

        // SFU skips seq 11..14 and delivers a fresh keyframe starting at seq 15 (2 packets).
        let mut obu2 = make_obu(6, b"keyframe");
        let tail = obu2.split_off(4);
        assert!(
            f.push(15, false, &make_rtp_packet(false, true, &[&obu2]))
                .is_none()
        );
        let u2 = f
            .push(16, true, &make_rtp_packet(true, false, &[&tail]))
            .expect("frame after inter-frame seq gap must not be dropped");
        assert_eq!(
            u2,
            vec![
                (6 << 3) | 0b10,
                8,
                b'k',
                b'e',
                b'y',
                b'f',
                b'r',
                b'a',
                b'm',
                b'e'
            ]
        );
    }

    #[test]
    fn append_sized_obu_preserves_already_sized_obus() {
        let already_sized = vec![(6 << 3) | 0b10, 2, b'h', b'i'];
        let mut out = Vec::new();
        append_sized_obu(&mut out, &already_sized);
        assert_eq!(out, already_sized);
    }

    #[test]
    fn av1_decoder_initializes_cleanly() {
        let _dec = Av1Decoder::new().expect("rav1d should initialize");
    }

    fn make_rtp_packet_w(z: bool, y: bool, w: u8, n: bool, elems: &[&[u8]]) -> Vec<u8> {
        let mut h = (w & 0b11) << 4;
        if z {
            h |= 0b1000_0000;
        }
        if y {
            h |= 0b0100_0000;
        }
        if n {
            h |= 0b0000_1000;
        }
        let mut out = vec![h];
        for (i, e) in elems.iter().enumerate() {
            let idx = (i + 1) as u8;
            if w == 0 || idx < w {
                write_leb128(&mut out, e.len());
            }
            out.extend_from_slice(e);
        }
        out
    }

    #[test]
    fn leb128_round_trips_multibyte_and_rejects_truncated_and_overflow() {
        for &val in &[0usize, 1, 127, 128, 300, 16_383, 16_384, 2_097_151] {
            let mut buf = Vec::new();
            write_leb128(&mut buf, val);
            assert_eq!(read_leb128(&buf), Some((val, buf.len())));
        }
        // Multi-byte lengths match expected LEB128 byte counts.
        let mut b128 = Vec::new();
        write_leb128(&mut b128, 128);
        assert_eq!(b128, vec![0x80, 0x01]);
        let mut b16384 = Vec::new();
        write_leb128(&mut b16384, 16_384);
        assert_eq!(b16384, vec![0x80, 0x80, 0x01]);

        // Empty and truncated varints return None.
        assert_eq!(read_leb128(&[]), None);
        assert_eq!(read_leb128(&[0x80]), None);
        assert_eq!(read_leb128(&[0x85, 0x80, 0xff]), None);

        // >8-byte varint overflow returns None.
        let overflow = [0x81u8; 9];
        assert_eq!(read_leb128(&overflow), None);
    }

    #[test]
    fn append_sized_obu_handles_extended_header_truncated_and_empty() {
        // Empty OBU is a no-op.
        let mut out = Vec::new();
        append_sized_obu(&mut out, &[]);
        assert!(out.is_empty());

        // Extended OBU header (`obu_extension_flag` bit 2 = 0x04) keeps 2 header bytes before size.
        let ext_header = (6u8 << 3) | 0b0000_0100;
        let ext_byte = 0b0010_1000;
        let ext_obu = vec![ext_header, ext_byte, b'a', b'b', b'c'];
        append_sized_obu(&mut out, &ext_obu);
        assert_eq!(
            out,
            vec![ext_header | 0b0000_0010, ext_byte, 3, b'a', b'b', b'c']
        );

        // Truncated extended OBU (only 1 byte present) is safely skipped.
        let mut out_trunc = Vec::new();
        append_sized_obu(&mut out_trunc, &[ext_header]);
        assert!(out_trunc.is_empty());

        // Multi-byte payload size (>127 bytes) encodes a 2-byte LEB128 length.
        let mut large_obu = vec![6u8 << 3];
        large_obu.extend(vec![0x42u8; 200]);
        let mut out_large = Vec::new();
        append_sized_obu(&mut out_large, &large_obu);
        assert_eq!(out_large[0], (6u8 << 3) | 0b10);
        assert_eq!(read_leb128(&out_large[1..]), Some((200, 2)));
        assert_eq!(out_large.len(), 1 + 2 + 200);
    }

    #[test]
    fn parse_packet_handles_w_1_2_3_multi_obu_and_trailing_y_continuation() {
        // W = 1: single OBU with no length prefix.
        let mut f = Av1Frames::default();
        let o1 = make_obu(1, b"seq");
        let u = f
            .push(1, true, &make_rtp_packet_w(false, false, 1, false, &[&o1]))
            .expect("W=1 unit");
        assert_eq!(u, vec![(1 << 3) | 0b10, 3, b's', b'e', b'q']);

        // W = 2: first OBU length-prefixed, second OBU consumes remainder.
        let o2 = make_obu(6, b"frm");
        let u = f
            .push(
                2,
                true,
                &make_rtp_packet_w(false, false, 2, false, &[&o1, &o2]),
            )
            .expect("W=2 unit");
        assert_eq!(
            u,
            vec![
                (1 << 3) | 0b10,
                3,
                b's',
                b'e',
                b'q',
                (6 << 3) | 0b10,
                3,
                b'f',
                b'r',
                b'm',
            ]
        );

        // W = 3 with trailing Y = 1 continuation into next packet (W = 1, Z = 1).
        let o3 = make_obu(3, b"hdr");
        let mut o4 = make_obu(6, b"splitbody");
        let o4_tail = o4.split_off(4);
        assert!(
            f.push(
                3,
                false,
                &make_rtp_packet_w(false, true, 3, false, &[&o1, &o3, &o4])
            )
            .is_none()
        );
        let u = f
            .push(
                4,
                true,
                &make_rtp_packet_w(true, false, 1, false, &[&o4_tail]),
            )
            .expect("W=3 + Y=1 continuation");
        let mut expected = Vec::new();
        append_sized_obu(&mut expected, &o1);
        append_sized_obu(&mut expected, &o3);
        append_sized_obu(&mut expected, &make_obu(6, b"splitbody"));
        assert_eq!(u, expected);

        // W = 2 with Y = 1 continuation on second element.
        let mut o5 = make_obu(6, b"abcdef");
        let o5_tail = o5.split_off(3);
        assert!(
            f.push(
                5,
                false,
                &make_rtp_packet_w(false, true, 2, false, &[&o1, &o5])
            )
            .is_none()
        );
        let u = f
            .push(
                6,
                true,
                &make_rtp_packet_w(true, false, 1, false, &[&o5_tail]),
            )
            .expect("W=2 + Y=1 continuation");
        let mut exp2 = Vec::new();
        append_sized_obu(&mut exp2, &o1);
        append_sized_obu(&mut exp2, &make_obu(6, b"abcdef"));
        assert_eq!(u, exp2);
    }

    #[test]
    fn av1_frames_mid_frame_gap_n_cvs_recovery_and_seq_wraparound() {
        let mut f = Av1Frames::default();
        let mut obu = make_obu(6, b"hello_world");
        let tail = obu.split_off(4);

        // Start a frame at seq 100 (Y=1, marker=false).
        assert!(
            f.push(100, false, &make_rtp_packet(false, true, &[&obu]))
                .is_none()
        );
        // Mid-frame seq gap (skip 101, arrive 102 with marker=false) -> broken = true.
        assert!(
            f.push(102, false, &make_rtp_packet(true, true, &[&tail]))
                .is_none()
        );
        assert!(f.broken);
        // Subsequent packet with marker=true discards the broken frame and resets.
        assert!(
            f.push(103, true, &make_rtp_packet(true, false, &[&tail]))
                .is_none()
        );
        assert!(!f.broken);

        // Next clean frame recovers immediately.
        let clean = make_obu(6, b"ok");
        assert!(
            f.push(104, true, &make_rtp_packet(false, false, &[&clean]))
                .is_some()
        );

        // Break a frame, then recover immediately on N=1 && Z=0 even before a marker clears `broken`.
        assert!(
            f.push(105, false, &make_rtp_packet(false, true, &[&obu]))
                .is_none()
        );
        assert!(
            f.push(107, false, &make_rtp_packet(true, false, &[&tail]))
                .is_none()
        );
        assert!(f.broken);
        // N=1 && Z=1 must NOT reset; stays broken.
        assert!(
            f.push(
                108,
                false,
                &make_rtp_packet_w(true, false, 1, true, &[&clean])
            )
            .is_none()
        );
        assert!(f.broken);
        // N=1 && Z=0 resets and delivers the new CVS keyframe cleanly!
        let cvs = f
            .push(
                109,
                true,
                &make_rtp_packet_w(false, false, 1, true, &[&clean]),
            )
            .expect("N=1 && Z=0 must recover from broken state");
        assert_eq!(cvs, vec![(6 << 3) | 0b10, 2, b'o', b'k']);

        // 16-bit RTP sequence wraparound: 65535 -> 0 mid-frame succeeds without triggering a gap.
        let mut f_wrap = Av1Frames::default();
        assert!(
            f_wrap
                .push(65535, false, &make_rtp_packet(false, true, &[&obu]))
                .is_none()
        );
        let wrapped = f_wrap
            .push(0, true, &make_rtp_packet(true, false, &[&tail]))
            .expect("65535 -> 0 must wrap seamlessly");
        let mut expected = Vec::new();
        append_sized_obu(&mut expected, &make_obu(6, b"hello_world"));
        assert_eq!(wrapped, expected);
    }

    #[test]
    fn av1_frames_protocol_violation_edge_cases() {
        let mut f = Av1Frames::default();
        let o1 = make_obu(6, b"abc");

        // Empty payload updates last_seq and returns None.
        assert!(f.push(1, false, &[]).is_none());
        assert_eq!(f.last_seq, Some(1));

        // Z=0 while partial is non-empty is a protocol violation -> marks broken and resets on marker.
        assert!(
            f.push(2, false, &make_rtp_packet(false, true, &[&o1]))
                .is_none()
        );
        assert!(
            f.push(3, true, &make_rtp_packet(false, false, &[&o1]))
                .is_none()
        );
        assert!(!f.broken);

        // Marker=true with Y=1 leaves non-empty partial -> discarded and reset.
        assert!(
            f.push(4, true, &make_rtp_packet(false, true, &[&o1]))
                .is_none()
        );
        assert!(f.partial.is_empty());
        assert!(f.unit.is_empty());

        // Truncated LEB128 in W=0 packet -> returns None, resets on marker.
        assert!(f.push(5, true, &[0x00, 0x80, 0x80]).is_none());
        assert!(!f.broken);

        // LEB128 length exceeding remaining bytes -> returns None.
        assert!(f.push(6, true, &[0x00, 50, 0x01, 0x02]).is_none());
    }

    #[test]
    fn picture_to_bgra_handles_i420_odd_i422_i444_i400_10bit_and_padded_strides() {
        use rav1d::include::dav1d::headers::Dav1dPixelLayout;

        #[allow(clippy::too_many_arguments)]
        unsafe fn build_pic(
            w: i32,
            h: i32,
            bpc: i32,
            layout: Dav1dPixelLayout,
            y_ptr: *const u8,
            u_ptr: Option<*const u8>,
            v_ptr: Option<*const u8>,
            y_stride: isize,
            c_stride: isize,
        ) -> Dav1dPicture {
            let mut pic = unsafe { std::mem::MaybeUninit::<Dav1dPicture>::zeroed().assume_init() };
            pic.p.w = w;
            pic.p.h = h;
            pic.p.bpc = bpc;
            pic.p.layout = layout;
            pic.data[0] = NonNull::new(y_ptr as *mut _);
            pic.data[1] = u_ptr.and_then(|p| NonNull::new(p as *mut _));
            pic.data[2] = v_ptr.and_then(|p| NonNull::new(p as *mut _));
            pic.stride[0] = y_stride;
            pic.stride[1] = c_stride;
            pic
        }

        // 1. 8-bit I420 fast path with padded strides (4x4, stride 8 / 4).
        let (w, h) = (4usize, 4usize);
        let y_stride = 8usize;
        let c_stride = 4usize;
        let mut y_buf = vec![0xAAu8; h * y_stride];
        let mut u_buf = vec![0xAAu8; (h / 2) * c_stride];
        let mut v_buf = vec![0xAAu8; (h / 2) * c_stride];
        for r in 0..h {
            for c in 0..w {
                y_buf[r * y_stride + c] = 235; // studio white Y
            }
        }
        for r in 0..(h / 2) {
            for c in 0..(w / 2) {
                u_buf[r * c_stride + c] = 128;
                v_buf[r * c_stride + c] = 128;
            }
        }
        let pic = unsafe {
            build_pic(
                w as i32,
                h as i32,
                8,
                DAV1D_PIXEL_LAYOUT_I420,
                y_buf.as_ptr(),
                Some(u_buf.as_ptr()),
                Some(v_buf.as_ptr()),
                y_stride as isize,
                c_stride as isize,
            )
        };
        let frame = unsafe { picture_to_bgra(&pic) }.expect("8-bit I420");
        let (fw, fh, bgra) = frame.into_bgra().unwrap();
        assert_eq!((fw, fh), (4, 4));
        for px in bgra.as_chunks::<4>().0 {
            assert_eq!(px, &[255, 255, 255, 255]);
        }

        // 2. Odd dimensions 5x3 I420 (exercises fallback path after 2x2 fast path).
        let (ow, oh) = (5usize, 3usize);
        let oy = vec![16u8; ow * oh]; // studio black Y=16
        let ou = vec![128u8; ow.div_ceil(2) * oh.div_ceil(2)];
        let ov = vec![128u8; ow.div_ceil(2) * oh.div_ceil(2)];
        let pic_odd = unsafe {
            build_pic(
                ow as i32,
                oh as i32,
                8,
                DAV1D_PIXEL_LAYOUT_I420,
                oy.as_ptr(),
                Some(ou.as_ptr()),
                Some(ov.as_ptr()),
                ow as isize,
                ow.div_ceil(2) as isize,
            )
        };
        let frame_odd = unsafe { picture_to_bgra(&pic_odd) }.expect("odd I420");
        let (fw, fh, bgra_odd) = frame_odd.into_bgra().unwrap();
        assert_eq!((fw, fh), (5, 3));
        for px in bgra_odd.as_chunks::<4>().0 {
            assert_eq!(px, &[0, 0, 0, 255]);
        }

        // 3. I422 (chroma half-width, full-height) with pure red chroma (Y=81, U=90, V=240).
        let y422 = [81u8; 4 * 2];
        let u422 = [90u8; 2 * 2];
        let v422 = [240u8; 2 * 2];
        let pic_422 = unsafe {
            build_pic(
                4,
                2,
                8,
                DAV1D_PIXEL_LAYOUT_I422,
                y422.as_ptr(),
                Some(u422.as_ptr()),
                Some(v422.as_ptr()),
                4,
                2,
            )
        };
        let (_, _, bgra_422) = unsafe { picture_to_bgra(&pic_422) }
            .expect("I422")
            .into_bgra()
            .unwrap();
        assert!(bgra_422[2] > 245 && bgra_422[1] < 10 && bgra_422[0] < 10 && bgra_422[3] == 255);

        // 4. I444 (full-res chroma) with pure blue (Y=41, U=240, V=110).
        let y444 = [41u8; 2 * 2];
        let u444 = [240u8; 2 * 2];
        let v444 = [110u8; 2 * 2];
        let pic_444 = unsafe {
            build_pic(
                2,
                2,
                8,
                DAV1D_PIXEL_LAYOUT_I444,
                y444.as_ptr(),
                Some(u444.as_ptr()),
                Some(v444.as_ptr()),
                2,
                2,
            )
        };
        let (_, _, bgra_444) = unsafe { picture_to_bgra(&pic_444) }
            .expect("I444")
            .into_bgra()
            .unwrap();
        assert!(bgra_444[0] > 245 && bgra_444[1] < 10 && bgra_444[2] < 10);

        // 5. I400 (grayscale, no chroma pointers).
        let y400 = [126u8; 3 * 2];
        let pic_400 = unsafe {
            build_pic(
                3,
                2,
                8,
                DAV1D_PIXEL_LAYOUT_I400,
                y400.as_ptr(),
                None,
                None,
                3,
                0,
            )
        };
        let (_, _, bgra_400) = unsafe { picture_to_bgra(&pic_400) }
            .expect("I400")
            .into_bgra()
            .unwrap();
        assert_eq!(bgra_400[0], bgra_400[1]);
        assert_eq!(bgra_400[1], bgra_400[2]);
        assert!(bgra_400[0] > 110 && bgra_400[0] < 145);

        // 6. 10-bit I420 shift path (`<< 2` matches 8-bit white).
        let y10 = [235u16 << 2; 4 * 2];
        let u10 = [128u16 << 2; 2];
        let v10 = [128u16 << 2; 2];
        let pic_10 = unsafe {
            build_pic(
                4,
                2,
                10,
                DAV1D_PIXEL_LAYOUT_I420,
                y10.as_ptr().cast(),
                Some(u10.as_ptr().cast()),
                Some(v10.as_ptr().cast()),
                (4 * 2) as isize,
                (2 * 2) as isize,
            )
        };
        let (_, _, bgra_10) = unsafe { picture_to_bgra(&pic_10) }
            .expect("10-bit I420")
            .into_bgra()
            .unwrap();
        for px in bgra_10.as_chunks::<4>().0 {
            assert_eq!(px, &[255, 255, 255, 255]);
        }

        // 7. Invalid dimensions or unsupported bpc return None.
        let pic_zero = unsafe {
            build_pic(
                0,
                4,
                8,
                DAV1D_PIXEL_LAYOUT_I400,
                y400.as_ptr(),
                None,
                None,
                0,
                0,
            )
        };
        assert!(unsafe { picture_to_bgra(&pic_zero) }.is_none());
        let pic_12bit = unsafe {
            build_pic(
                2,
                2,
                12,
                DAV1D_PIXEL_LAYOUT_I400,
                y400.as_ptr(),
                None,
                None,
                4,
                0,
            )
        };
        assert!(unsafe { picture_to_bgra(&pic_12bit) }.is_none());
    }
}
