use crate::ImageKind;

fn lower_hex(s: &str, n: usize) -> bool {
    s.len() == n
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn is_note_id(s: &str) -> bool {
    s.strip_prefix("note_")
        .is_some_and(|suffix| lower_hex(suffix, 16))
}
pub fn is_image_id(s: &str) -> bool {
    lower_hex(s, 16)
}
pub fn is_note_token(s: &str) -> bool {
    lower_hex(s, 32)
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 15) as usize] as char);
    }
    out
}

pub fn note_id_from_bytes(b: [u8; 8]) -> String {
    format!("note_{}", hex(&b))
}
pub fn image_id_from_bytes(b: [u8; 8]) -> String {
    hex(&b)
}
pub fn token_from_bytes(b: [u8; 16]) -> String {
    hex(&b)
}
/// Callers validate both ids before using the generated key.
pub fn r2_key(note_id: &str, image_id: &str, kind: ImageKind) -> String {
    format!("{note_id}/{image_id}.{}", kind.ext())
}
