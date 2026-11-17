use note_core::{
    image_id_from_bytes, is_image_id, is_note_id, is_note_token, note_id_from_bytes, r2_key, scrub,
    sniff, token_from_bytes, ImageKind, ScrubError,
};

const IPHONE: &[u8] = include_bytes!("fixtures/iphone_portrait_gps.jpg");
const MIXED: &[u8] = include_bytes!("fixtures/jfif_icc_xmp_iptc.jpg");
const LATE: &[u8] = include_bytes!("fixtures/late_exif.png");
const TEXT: &[u8] = include_bytes!("fixtures/text_chunks.png");

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn sniff_magic_only() {
    assert_eq!(sniff(LATE), Some(ImageKind::Png));
    assert_eq!(sniff(IPHONE), Some(ImageKind::Jpeg));
    for b in [
        include_bytes!("fixtures/fake.heic").as_slice(),
        include_bytes!("fixtures/svg_as_png.png").as_slice(),
        b"<html>".as_slice(),
    ] {
        assert_eq!(sniff(b), None);
        assert!(matches!(scrub(b), Err(ScrubError::UnsupportedFormat)));
    }
}

#[test]
fn iphone_portrait_keeps_orientation_drops_gps() {
    let (_, clean) = scrub(IPHONE).unwrap();
    assert!(!contains(&clean, b"Apple iOS"));
    assert!(!contains(&clean, &[0x25, 0x88]));
    assert!(!contains(&clean, &[0x88, 0x25]));
    assert_eq!(clean.windows(6).filter(|w| *w == b"Exif\0\0").count(), 1);
    let exif = clean.windows(6).position(|w| w == b"Exif\0\0").unwrap();
    let tiff = &clean[exif + 6..];
    assert_eq!(&tiff[..4], b"MM\0*");
    assert_eq!(u16::from_be_bytes([tiff[8], tiff[9]]), 1);
    assert_eq!(u16::from_be_bytes([tiff[10], tiff[11]]), 0x0112);
    assert_eq!(u16::from_be_bytes([tiff[18], tiff[19]]), 6);
    assert!(!contains(&clean, b"JFIF"));
}

#[test]
fn drops_xmp_and_iptc() {
    let (_, clean) = scrub(MIXED).unwrap();
    assert!(!contains(&clean, b"http://ns.adobe.com/xap/1.0/"));
    assert!(!contains(&clean, b"Photoshop 3.0"));
    assert!(contains(&clean, b"ICC_PROFILE\0"));
    assert!(contains(&clean, b"JFIF\0"));
}

#[test]
fn jfif_thumbnail_and_trailing_data_dropped() {
    // Replace the fixture's ordinary JFIF APP0 with one carrying a 1x1 RGB
    // thumbnail and surplus private bytes after it.
    assert_eq!(&MIXED[2..4], &[0xff, 0xe0]);
    let old_end = 4 + u16::from_be_bytes([MIXED[4], MIXED[5]]) as usize;
    let mut payload = MIXED[6..18].to_vec(); // identifier, version, units, densities
    payload.extend_from_slice(&[1, 1, 0x91, 0x82, 0x73]);
    payload.extend_from_slice(b"CUT_FRAME_SECRET");
    let mut jpeg = MIXED[..4].to_vec();
    jpeg.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    jpeg.extend_from_slice(&payload);
    jpeg.extend_from_slice(&MIXED[old_end..]);

    let (_, clean) = scrub(&jpeg).unwrap();
    let jfif = clean.windows(5).position(|w| w == b"JFIF\0").unwrap();
    let segment = jfif - 4;
    assert_eq!(&clean[segment..segment + 4], &[0xff, 0xe0, 0, 16]);
    assert_eq!(&clean[jfif..jfif + 12], &payload[..12]);
    assert_eq!(&clean[jfif + 12..jfif + 14], &[0, 0]);
    assert!(!contains(&clean, &[0x91, 0x82, 0x73]));
    assert!(!contains(&clean, b"CUT_FRAME_SECRET"));
}

#[test]
fn orientation_zero_is_unrotated_but_nine_rejected() {
    let exif = IPHONE.windows(6).position(|w| w == b"Exif\0\0").unwrap();
    let value = exif + 6 + 18; // little-endian SHORT in IFD0
    let mut zero = IPHONE.to_vec();
    zero[value..value + 2].copy_from_slice(&0u16.to_le_bytes());
    let (_, clean) = scrub(&zero).unwrap();
    assert!(!contains(&clean, b"Exif\0\0"));
    assert!(!contains(&clean, b"Apple iOS"));

    zero[value..value + 2].copy_from_slice(&9u16.to_le_bytes());
    assert!(matches!(scrub(&zero), Err(ScrubError::Malformed(_))));
}

#[test]
fn jpeg_image_data_unchanged() {
    let (_, clean) = scrub(IPHONE).unwrap();
    fn scan(bytes: &[u8]) -> &[u8] {
        let sos = bytes.windows(2).position(|w| w == [0xff, 0xda]).unwrap();
        &bytes[sos..]
    }
    assert_eq!(scan(IPHONE), scan(&clean));
}

fn png_chunks(bytes: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut pos = 8;
    let mut chunks = vec![];
    while pos < bytes.len() {
        let len = u32::from_be_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
        let name: [u8; 4] = bytes[pos + 4..pos + 8].try_into().unwrap();
        chunks.push((name, &bytes[pos..pos + len + 12]));
        pos += len + 12;
    }
    chunks
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

#[test]
fn png_late_exif_dropped() {
    let (_, clean) = scrub(LATE).unwrap();
    assert!(png_chunks(LATE).iter().any(|(n, _)| n == b"eXIf"));
    assert!(!png_chunks(&clean).iter().any(|(n, _)| n == b"eXIf"));
    assert!(!contains(&clean, b"Apple iOS"));
}

#[test]
fn png_text_chunks_dropped() {
    let (_, clean) = scrub(TEXT).unwrap();
    for name in [b"tEXt", b"iTXt", b"zTXt", b"tIME"] {
        assert!(!png_chunks(&clean).iter().any(|(n, _)| n == name));
    }
    assert!(!contains(&clean, b"Tokyo"));
}

#[test]
fn png_crc_valid() {
    let (_, clean) = scrub(LATE).unwrap();
    for (_, chunk) in png_chunks(&clean) {
        assert_eq!(
            crc32(&chunk[4..chunk.len() - 4]),
            u32::from_be_bytes(chunk[chunk.len() - 4..].try_into().unwrap())
        );
    }
}

#[test]
fn truncated_rejected() {
    for b in [
        include_bytes!("fixtures/truncated.jpg").as_slice(),
        &IPHONE[..20],
        &LATE[..LATE.len() - 3],
    ] {
        assert!(matches!(scrub(b), Err(ScrubError::Malformed(_))));
    }
    let mut bad = LATE.to_vec();
    bad[29] ^= 1;
    assert!(matches!(scrub(&bad), Err(ScrubError::Malformed(_))));
    let mut zero_width = LATE.to_vec();
    zero_width[16..20].copy_from_slice(&0u32.to_be_bytes());
    let crc = crc32(&zero_width[12..29]);
    zero_width[29..33].copy_from_slice(&crc.to_be_bytes());
    assert!(matches!(scrub(&zero_width), Err(ScrubError::Malformed(_))));
    let mut trailing = IPHONE.to_vec();
    trailing.extend_from_slice(b"\xff\xd8\xff\xe1hidden GPS secondary image");
    let (_, out) = scrub(&trailing).expect("data after EOI is dropped, not rejected");
    assert!(!contains(&out, b"hidden GPS"));
    assert!(out.ends_with(&[0xff, 0xd9]));
    let mut bad_exif = IPHONE.to_vec();
    let exif = bad_exif.windows(6).position(|w| w == b"Exif\0\0").unwrap();
    bad_exif[exif + 6] = b'X';
    assert!(matches!(scrub(&bad_exif), Err(ScrubError::Malformed(_))));
}

#[test]
fn ids_format() {
    let note = note_id_from_bytes([0xab; 8]);
    let image = image_id_from_bytes([0xcd; 8]);
    let token = token_from_bytes([0xef; 16]);
    assert_eq!(note, "note_abababababababab");
    assert_eq!(image, "cdcdcdcdcdcdcdcd");
    assert_eq!(token, "ef".repeat(16));
    assert!(is_note_id(&note) && is_image_id(&image) && is_note_token(&token));
    for bad in [
        "Note_abababababababab",
        "note_ABABABABABABABAB",
        "note_abab",
        "nope_abababababababab",
    ] {
        assert!(!is_note_id(bad));
    }
    assert!(!is_image_id("CDCDCDCDCDCDCDCD"));
    assert!(!is_note_token("ef"));
    assert_eq!(
        r2_key(&note, &image, ImageKind::Png),
        format!("{note}/{image}.png")
    );
}
