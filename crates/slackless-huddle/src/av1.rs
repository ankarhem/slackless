//! Pure-Rust AV1 RTP depacketizer and `rav1d` AV1 video decoder for incoming Slack Huddle
//! screen shares and video streams (official Slack clients publish `#content` screen shares in AV1).

use std::ptr::NonNull;
use std::sync::Arc;

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
}

impl Av1Frames {
    /// Push one RTP packet payload; returns `Some(temporal_unit)` when `marker` closes a valid unit.
    pub fn push(&mut self, seq: u16, marker: bool, payload: &[u8]) -> Option<Vec<u8>> {
        let Some(&header) = payload.first() else {
            self.last_seq = Some(seq);
            return None;
        };
        let z_continues_prev = header & 0b1000_0000 != 0;
        let n_new_cvs = header & 0b0000_1000 != 0;

        let seq_gap = self
            .last_seq
            .is_some_and(|prev| seq.wrapping_sub(prev) != 1);
        self.last_seq = Some(seq);

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

/// Convert an 8-bit or 10-bit `Dav1dPicture` (`I420`, `I422`, `I444`, or `I400`) into a BGRA8888
/// [`VideoFrame`] for GPUI.
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
    let shift16 = (bpc - 8) as u32;

    let mut bgra = vec![0u8; w * h * 4];
    for row in 0..h {
        let y_row = (row as isize) * y_stride;
        let c_row = ((row >> ch_shift) as isize) * c_stride;
        for col in 0..w {
            let (y, u, v) = if bpc == 8 {
                let y = unsafe { *y_ptr.offset(y_row + col as isize) } as f32;
                let (u, v) = match (u_ptr, v_ptr) {
                    (Some(up), Some(vp)) => {
                        let cx = (col >> cw_shift) as isize;
                        (
                            unsafe { *up.offset(c_row + cx) } as f32 - 128.0,
                            unsafe { *vp.offset(c_row + cx) } as f32 - 128.0,
                        )
                    }
                    _ => (0.0, 0.0),
                };
                (y, u, v)
            } else {
                let y16 = unsafe { *(y_ptr.offset(y_row) as *const u16).add(col) };
                let y = (y16 >> shift16) as f32;
                let (u, v) = match (u_ptr, v_ptr) {
                    (Some(up), Some(vp)) => {
                        let cx = col >> cw_shift;
                        let u16v = unsafe { *(up.offset(c_row) as *const u16).add(cx) };
                        let v16v = unsafe { *(vp.offset(c_row) as *const u16).add(cx) };
                        (
                            (u16v >> shift16) as f32 - 128.0,
                            (v16v >> shift16) as f32 - 128.0,
                        )
                    }
                    _ => (0.0, 0.0),
                };
                (y, u, v)
            };
            let yy = (y - 16.0) * 1.164;
            let r = (yy + 1.596 * v).clamp(0.0, 255.0) as u8;
            let g = (yy - 0.392 * u - 0.813 * v).clamp(0.0, 255.0) as u8;
            let b = (yy + 2.017 * u).clamp(0.0, 255.0) as u8;
            let out = (row * w + col) * 4;
            bgra[out] = b;
            bgra[out + 1] = g;
            bgra[out + 2] = r;
            bgra[out + 3] = 255;
        }
    }
    Some(VideoFrame {
        width: w as u32,
        height: h as u32,
        bgra: Arc::new(bgra),
    })
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
}
