//! A TrueType font re-registered under its own family with its em grown, so its glyphs come
//! out smaller at a given font size. GPUI shapes a paragraph at one size, so a smaller face
//! is how one run (inline code) gets smaller glyphs (see `theme::INLINE_CODE_FAMILY`).

/// Size-specific tables (bitmap strikes, device metrics): stale once the em changes.
const DROPPED: [&[u8; 4]; 8] = [
    b"bdat", b"bloc", b"EBDT", b"EBLC", b"hdmx", b"VDMX", b"LTSH", b"fond",
];

/// `font` (a TrueType sfnt) renamed to `family` with its glyphs scaled by `scale`: the head
/// table's units per em grow by 1/`scale`, and every outline and metric is measured in
/// them. `None` when the data is not an sfnt with a head table.
pub fn scaled(font: &[u8], family: &str, scale: f32) -> Option<Vec<u8>> {
    let count = usize::from(u16::from_be_bytes(font.get(4..6)?.try_into().ok()?));
    let mut tables = Vec::with_capacity(count);
    for record in font.get(12..12 + 16 * count)?.as_chunks::<16>().0 {
        let tag: [u8; 4] = record[..4].try_into().ok()?;
        if DROPPED.contains(&&tag) {
            continue;
        }
        let offset = u32::from_be_bytes(record[8..12].try_into().ok()?) as usize;
        let len = u32::from_be_bytes(record[12..16].try_into().ok()?) as usize;
        let mut data = font.get(offset..offset.checked_add(len)?)?.to_vec();
        match &tag {
            b"head" => {
                let em = u16::from_be_bytes(data.get(18..20)?.try_into().ok()?);
                let grown = (f32::from(em) / scale).round().clamp(16., 16384.) as u16;
                data[18..20].copy_from_slice(&grown.to_be_bytes());
            }
            b"name" => data = name_table(family),
            _ => {}
        }
        tables.push((tag, data));
    }
    sfnt(font.get(..4)?.try_into().ok()?, tables)
}

/// An sfnt file of `tables`, its directory sorted by tag and every checksum set.
fn sfnt(version: [u8; 4], mut tables: Vec<([u8; 4], Vec<u8>)>) -> Option<Vec<u8>> {
    tables.sort_by_key(|(tag, _)| *tag);
    let count = tables.len();
    let pow = 1usize << count.checked_ilog2()?;
    let mut out = version.to_vec();
    for field in [count, 16 * pow, pow.ilog2() as usize, 16 * (count - pow)] {
        out.extend_from_slice(&(field as u16).to_be_bytes());
    }
    let data_start = 12 + 16 * count;
    let mut body = Vec::new();
    let mut head = None;
    for (tag, data) in &mut tables {
        if tag == b"head" {
            // checkSumAdjustment counts as zero in every checksum until the file is whole.
            data.get_mut(8..12)?.fill(0);
            head = Some(data_start + body.len());
        }
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum(data).to_be_bytes());
        out.extend_from_slice(&((data_start + body.len()) as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        body.extend_from_slice(data);
        body.resize(body.len().next_multiple_of(4), 0);
    }
    out.extend(body);
    let head = head?;
    let adjustment = 0xB1B0_AFBA_u32.wrapping_sub(checksum(&out));
    out[head + 8..head + 12].copy_from_slice(&adjustment.to_be_bytes());
    Some(out)
}

/// The sum of `data` as big-endian u32 words, zero-padded.
fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, chunk| {
        let mut word = [0; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

/// A name table naming `family` (Regular) in Mac Roman and Windows Unicode records: the
/// family, subfamily, unique, full and PostScript names.
fn name_table(family: &str) -> Vec<u8> {
    let postscript: String = family.chars().filter(|c| !c.is_whitespace()).collect();
    let names: [(u16, &str); 5] = [
        (1, family),
        (2, "Regular"),
        (3, family),
        (4, family),
        (6, &postscript),
    ];
    let mut records = Vec::new();
    let mut strings: Vec<u8> = Vec::new();
    for (platform, encoding, language) in [(1u16, 0u16, 0u16), (3, 1, 0x409)] {
        for (id, value) in names {
            let bytes: Vec<u8> = if platform == 1 {
                value.bytes().collect()
            } else {
                value.encode_utf16().flat_map(u16::to_be_bytes).collect()
            };
            for field in [
                platform,
                encoding,
                language,
                id,
                bytes.len() as u16,
                strings.len() as u16,
            ] {
                records.extend_from_slice(&field.to_be_bytes());
            }
            strings.extend(bytes);
        }
    }
    let count = 2 * names.len() as u16;
    let mut table = Vec::new();
    for field in [0, count, 6 + 12 * count] {
        table.extend_from_slice(&field.to_be_bytes());
    }
    table.extend(records);
    table.extend(strings);
    table
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn tables_of(font: &[u8]) -> HashMap<[u8; 4], Vec<u8>> {
        let count = usize::from(u16::from_be_bytes([font[4], font[5]]));
        font[12..12 + 16 * count]
            .as_chunks::<16>()
            .0
            .iter()
            .map(|record| {
                let offset = u32::from_be_bytes(record[8..12].try_into().unwrap()) as usize;
                let len = u32::from_be_bytes(record[12..16].try_into().unwrap()) as usize;
                (
                    record[..4].try_into().unwrap(),
                    font[offset..offset + len].to_vec(),
                )
            })
            .collect()
    }

    #[test]
    fn scaling_grows_the_em_renames_and_keeps_the_file_valid() {
        let mut head = vec![0u8; 54];
        head[18..20].copy_from_slice(&2048u16.to_be_bytes());
        let font = sfnt(
            [0, 1, 0, 0],
            vec![
                (*b"head", head),
                (*b"name", vec![0; 6]),
                (*b"hdmx", vec![1; 8]),
            ],
        )
        .unwrap();

        let scaled = scaled(&font, "Test Face", 0.8).unwrap();

        let tables = tables_of(&scaled);
        assert!(!tables.contains_key(b"hdmx"), "size-specific tables go");
        let head = &tables[b"head"];
        assert_eq!(u16::from_be_bytes([head[18], head[19]]), 2560);
        let family: Vec<u8> = "Test Face"
            .encode_utf16()
            .flat_map(u16::to_be_bytes)
            .collect();
        assert!(
            tables[b"name"]
                .windows(family.len())
                .any(|window| window == family),
            "the Windows family record names the new face"
        );
        assert_eq!(
            checksum(&scaled),
            0xB1B0_AFBA,
            "the whole-file checksum holds"
        );
    }
}
