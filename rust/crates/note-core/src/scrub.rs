#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Jpeg,
    Png,
}

impl ImageKind {
    pub fn ext(&self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
        }
    }
    pub fn content_type(&self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrubError {
    UnsupportedFormat,
    Malformed(&'static str),
}

pub fn sniff(bytes: &[u8]) -> Option<ImageKind> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(ImageKind::Jpeg)
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageKind::Png)
    } else {
        None
    }
}

pub fn scrub(bytes: &[u8]) -> Result<(ImageKind, Vec<u8>), ScrubError> {
    match sniff(bytes) {
        Some(ImageKind::Jpeg) => scrub_jpeg(bytes).map(|v| (ImageKind::Jpeg, v)),
        Some(ImageKind::Png) => scrub_png(bytes).map(|v| (ImageKind::Png, v)),
        None => Err(ScrubError::UnsupportedFormat),
    }
}

fn be16(b: &[u8]) -> usize {
    u16::from_be_bytes([b[0], b[1]]) as usize
}
fn be32(b: &[u8]) -> usize {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize
}

fn orientation(exif: &[u8]) -> Result<Option<u16>, ScrubError> {
    let t = exif
        .strip_prefix(b"Exif\0\0")
        .ok_or(ScrubError::Malformed("EXIF signature"))?;
    if t.len() < 8 {
        return Err(ScrubError::Malformed("TIFF header"));
    }
    let little = match &t[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return Err(ScrubError::Malformed("TIFF byte order")),
    };
    let u16_at = |p: usize| -> Option<u16> {
        let b = t.get(p..p.checked_add(2)?)?;
        Some(if little {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    };
    let u32_at = |p: usize| -> Option<usize> {
        let b = t.get(p..p.checked_add(4)?)?;
        Some(if little {
            u32::from_le_bytes(b.try_into().ok()?)
        } else {
            u32::from_be_bytes(b.try_into().ok()?)
        } as usize)
    };
    if u16_at(2) != Some(42) {
        return Err(ScrubError::Malformed("TIFF magic"));
    }
    let ifd = u32_at(4).ok_or(ScrubError::Malformed("IFD offset"))?;
    let count = u16_at(ifd).ok_or(ScrubError::Malformed("IFD count"))? as usize;
    let entries = ifd
        .checked_add(2)
        .and_then(|x| x.checked_add(count.checked_mul(12)?))
        .ok_or(ScrubError::Malformed("IFD size"))?;
    let entries_end = entries
        .checked_add(4)
        .ok_or(ScrubError::Malformed("IFD entries"))?;
    if t.get(entries..entries_end).is_none() {
        return Err(ScrubError::Malformed("IFD entries"));
    }
    let mut found = None;
    for i in 0..count {
        let p = ifd + 2 + i * 12;
        if u16_at(p) == Some(0x0112) {
            if found.is_some() || u16_at(p + 2) != Some(3) || u32_at(p + 4) != Some(1) {
                return Err(ScrubError::Malformed("orientation tag"));
            }
            let value = u16_at(p + 8).ok_or(ScrubError::Malformed("orientation value"))?;
            if value > 8 {
                return Err(ScrubError::Malformed("orientation range"));
            }
            // Some cameras write 0 for the default orientation. Treat it as
            // unrotated, and omit the synthetic EXIF segment below.
            found = Some(value.max(1));
        }
    }
    Ok(found)
}

fn minimal_orientation(value: u16) -> Vec<u8> {
    // Big-endian TIFF with one IFD0 entry and no other tags or linked IFDs.
    let mut payload = b"Exif\0\0MM\0*\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01".to_vec();
    payload.extend_from_slice(&value.to_be_bytes());
    payload.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    let mut segment = vec![0xff, 0xe1];
    segment.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    segment.extend_from_slice(&payload);
    segment
}

fn jfif_without_thumbnail(payload: &[u8]) -> Result<[u8; 18], ScrubError> {
    // JFIF: identifier (5), version (2), units (1), densities (4),
    // thumbnail dimensions (2), then 3 bytes per thumbnail pixel.
    if payload.len() < 14 {
        return Err(ScrubError::Malformed("JFIF header"));
    }
    let thumbnail_len = 3 * payload[12] as usize * payload[13] as usize;
    if payload.len() < 14 + thumbnail_len {
        return Err(ScrubError::Malformed("JFIF thumbnail"));
    }
    let mut segment = [0u8; 18];
    segment[..4].copy_from_slice(&[0xff, 0xe0, 0, 16]);
    segment[4..18].copy_from_slice(&payload[..14]);
    segment[16..18].copy_from_slice(&[0, 0]);
    Ok(segment)
}

fn scrub_jpeg(b: &[u8]) -> Result<Vec<u8>, ScrubError> {
    let mut out = vec![0xff, 0xd8];
    let mut pos = 2;
    let mut orient = None;
    let mut have_sof = false;
    let mut have_scan = false;
    let mut have_dqt = false;
    let mut have_dht = false;
    while pos < b.len() {
        if b[pos] != 0xff {
            return Err(ScrubError::Malformed("JPEG marker"));
        }
        let marker_start = pos;
        while b.get(pos) == Some(&0xff) {
            pos += 1;
        }
        let marker = *b.get(pos).ok_or(ScrubError::Malformed("JPEG marker end"))?;
        pos += 1;
        if marker == 0x00 || marker == 0xd8 {
            return Err(ScrubError::Malformed("JPEG marker type"));
        }
        if marker == 0xd9 {
            if !have_sof || !have_scan || !have_dqt || !have_dht {
                return Err(ScrubError::Malformed("JPEG incomplete"));
            }
            // Bytes after the first EOI are dropped, not copied: phone cameras
            // append MPF secondary images / HDR gain maps there (each with its
            // own EXIF). Rejecting them would refuse ordinary phone photos.
            // The orientation segment goes before all frame and scan markers.
            if let Some(v) = orient.filter(|v| *v != 1) {
                out.splice(2..2, minimal_orientation(v));
            }
            out.extend_from_slice(&b[marker_start..pos]);
            return Ok(out);
        }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            return Err(ScrubError::Malformed("JPEG marker outside scan"));
        }
        let len_end = pos
            .checked_add(2)
            .ok_or(ScrubError::Malformed("JPEG length"))?;
        let len_bytes = b
            .get(pos..len_end)
            .ok_or(ScrubError::Malformed("JPEG length"))?;
        let len = be16(len_bytes);
        if len < 2 {
            return Err(ScrubError::Malformed("JPEG length"));
        }
        let end = pos
            .checked_add(len)
            .ok_or(ScrubError::Malformed("JPEG segment bounds"))?;
        let payload = b
            .get(pos + 2..end)
            .ok_or(ScrubError::Malformed("JPEG segment bounds"))?;
        let keep = match marker {
            0xdb => {
                have_dqt = true;
                true
            }
            0xc4 => {
                have_dht = true;
                true
            }
            0xdd => true,
            0xc0..=0xcf if !matches!(marker, 0xc4 | 0xc8 | 0xcc) => {
                have_sof = true;
                true
            }
            0xda => {
                if !have_sof {
                    return Err(ScrubError::Malformed("SOS before SOF"));
                }
                have_scan = true;
                true
            }
            0xe0 => {
                if payload.starts_with(b"JFIF\0") {
                    out.extend_from_slice(&jfif_without_thumbnail(payload)?);
                }
                false
            }
            0xe2 => payload.starts_with(b"ICC_PROFILE\0"),
            0xe1 => {
                if payload.starts_with(b"Exif\0\0") {
                    let next = orientation(payload)?;
                    if next.is_some() && orient.is_some() {
                        return Err(ScrubError::Malformed("multiple orientations"));
                    }
                    orient = next.or(orient);
                }
                false
            }
            0xe3..=0xef | 0xfe => false,
            _ => return Err(ScrubError::Malformed("unsupported JPEG marker")),
        };
        if keep {
            out.extend_from_slice(&b[marker_start..end]);
        }
        pos = end;
        if marker == 0xda {
            let start = pos;
            while pos < b.len() {
                if b[pos] != 0xff {
                    pos += 1;
                    continue;
                }
                let mut next = pos + 1;
                while b.get(next) == Some(&0xff) {
                    next += 1;
                }
                match b.get(next).copied() {
                    Some(0x00) | Some(0xd0..=0xd7) => pos = next + 1,
                    Some(_) => break,
                    None => return Err(ScrubError::Malformed("unterminated scan")),
                }
            }
            if pos == b.len() {
                return Err(ScrubError::Malformed("missing EOI"));
            }
            out.extend_from_slice(&b[start..pos]);
        }
    }
    Err(ScrubError::Malformed("missing EOI"))
}

fn crc32(b: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in b {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

fn scrub_png(b: &[u8]) -> Result<Vec<u8>, ScrubError> {
    let mut out = b[..8].to_vec();
    let mut pos = 8usize;
    let mut seen_ihdr = false;
    let mut seen_idat = false;
    let mut seen_iend = false;
    while pos < b.len() {
        let head = b
            .get(pos..pos + 8)
            .ok_or(ScrubError::Malformed("PNG chunk header"))?;
        let len = be32(head);
        let end = pos
            .checked_add(12)
            .and_then(|n| n.checked_add(len))
            .ok_or(ScrubError::Malformed("PNG chunk size"))?;
        let chunk = b
            .get(pos..end)
            .ok_or(ScrubError::Malformed("PNG chunk bounds"))?;
        let name = &head[4..8];
        if !name.iter().all(|c| c.is_ascii_alphabetic()) || name[2].is_ascii_lowercase() {
            return Err(ScrubError::Malformed("PNG chunk name"));
        }
        if crc32(&chunk[4..chunk.len() - 4]) as usize != be32(&chunk[chunk.len() - 4..]) {
            return Err(ScrubError::Malformed("PNG CRC"));
        }
        if !seen_ihdr && (name != b"IHDR" || len != 13) {
            return Err(ScrubError::Malformed("PNG IHDR"));
        }
        let keep = match name {
            b"IHDR" if !seen_ihdr => {
                let data = &chunk[8..21];
                if be32(&data[..4]) == 0
                    || be32(&data[4..8]) == 0
                    || data[10] != 0
                    || data[11] != 0
                    || data[12] > 1
                {
                    return Err(ScrubError::Malformed("PNG IHDR values"));
                }
                seen_ihdr = true;
                true
            }
            b"PLTE" | b"tRNS" | b"gAMA" | b"cHRM" | b"sRGB" | b"iCCP" | b"bKGD" | b"pHYs"
            | b"sBIT" => true,
            b"IDAT" => {
                seen_idat = true;
                true
            }
            b"IEND" if len == 0 && seen_idat => {
                seen_iend = true;
                true
            }
            _ if name[0].is_ascii_lowercase() => false,
            _ => return Err(ScrubError::Malformed("unsupported PNG critical chunk")),
        };
        if keep {
            out.extend_from_slice(chunk);
        }
        pos = end;
        if seen_iend {
            if pos != b.len() {
                return Err(ScrubError::Malformed("PNG trailing bytes"));
            }
            return Ok(out);
        }
    }
    Err(ScrubError::Malformed("PNG missing IEND"))
}
