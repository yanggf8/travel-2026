/// Generate a fresh run-id (UUIDv4-shaped). Volatile — not for dedup keys.
pub fn new_run_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        ^ (n as u128);
    let p1 = (nanos & 0xFFFF_FFFF) as u32;
    let p2 = ((nanos >> 32) & 0xFFFF) as u16;
    let p3 = ((nanos >> 48) & 0x0FFF) as u16;
    let p4 = 0x8000 | (((nanos >> 60) & 0x3FFF) as u16);
    // 48-bit final segment: {:012x} is a MINIMUM width, so an unmasked u64 emits up to 16 hex
    // digits (a 40-char, non-RFC-4122 id). Mask to 12 hex digits to keep the canonical 36 chars.
    let p5 = ((nanos as u64) ^ 0xDEAD_BEEF_CAFE_F00D) & 0xFFFF_FFFF_FFFF;
    format!("{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}", p1, p2, p3, p4, p5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_id_is_canonical_36_char_uuid_shape() {
        for _ in 0..1000 {
            let id = new_run_id();
            assert_eq!(id.len(), 36, "expected 36-char UUID, got {id:?}");
            let parts: Vec<&str> = id.split('-').collect();
            assert_eq!(parts.len(), 5, "expected 5 hyphen groups in {id:?}");
            let widths: Vec<usize> = parts.iter().map(|p| p.len()).collect();
            assert_eq!(widths, vec![8, 4, 4, 4, 12], "group widths wrong in {id:?}");
            assert!(parts[2].starts_with('4'), "version nibble not 4 in {id:?}");
            assert!(
                id.chars().all(|c| c == '-' || c.is_ascii_hexdigit()),
                "non-hex char in {id:?}"
            );
        }
    }
}