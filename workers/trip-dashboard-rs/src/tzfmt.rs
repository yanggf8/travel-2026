//! Format a UTC ISO-8601 instant in an IANA timezone ("MM/DD HH:MM") for the weather
//! "updated" stamp. The timezone is `destination_config.timezone` (content, never
//! hardcoded). The parse is pure and host-tested; the zone conversion uses the Workers
//! runtime's `Intl.DateTimeFormat` (wasm only) and is verified on deploy. Anything
//! missing, unparseable or unsupported yields None — callers omit the stamp, no fallback.

/// `2026-11-14T23:39:12.000Z` (or `±hh:mm` offset) -> Unix epoch milliseconds.
pub fn iso_to_epoch_ms(iso: &str) -> Option<f64> {
    let b = iso.trim().as_bytes();
    if b.len() < 17 || b[4] != b'-' || b[7] != b'-' || !(b[10] == b'T' || b[10] == b' ') || b[13] != b':' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let part = b.get(r)?;
        if part.iter().all(u8::is_ascii_digit) {
            std::str::from_utf8(part).ok()?.parse().ok()
        } else {
            None
        }
    };
    let (y, mo, d, h, mi) = (num(0..4)?, num(5..7)?, num(8..10)?, num(11..13)?, num(14..16)?);
    if !(1..=12).contains(&mo) || d < 1 || d > crate::render::now::days_in_month(y, mo) || h > 23 || mi > 59 {
        return None;
    }
    let mut i = 16;
    while i < b.len() && (b[i] == b':' || b[i] == b'.' || b[i].is_ascii_digit()) {
        i += 1;
    }
    let off_min = match b.get(i) {
        Some(b'Z') if i + 1 == b.len() => 0,
        Some(sign @ (b'+' | b'-')) if i + 6 == b.len() && b[i + 3] == b':' => {
            let v = num(i + 1..i + 3)? * 60 + num(i + 4..i + 6)?;
            if *sign == b'-' { -v } else { v }
        }
        _ => return None,
    };
    let yy = if mo <= 2 { y - 1 } else { y };
    let era = if yy >= 0 { yy } else { yy - 399 } / 400;
    let yoe = yy - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some((((days * 24 + h) * 60 + mi - off_min) * 60_000) as f64)
}

/// Assemble "MM/DD HH:MM" from `Intl` parts (type, value). None if a part is missing.
pub fn stamp_from_parts(parts: &[(String, String)]) -> Option<String> {
    let get = |k: &str| parts.iter().find(|(t, _)| t == k).map(|(_, v)| v.as_str()).filter(|v| !v.is_empty());
    let hour = get("hour")?;
    // Some engines render midnight as "24" under hour12:false; normalise.
    let hour = if hour == "24" { "00" } else { hour };
    Some(format!("{}/{} {}:{}", get("month")?, get("day")?, hour, get("minute")?))
}

/// The UTC ISO instant in IANA zone `tz`, "MM/DD HH:MM". None when `tz` is empty,
/// the instant does not parse, or the runtime rejects the zone.
#[cfg(target_arch = "wasm32")]
pub fn format_in_tz(iso: &str, tz: &str) -> Option<String> {
    use worker::js_sys::{Array, Date, Function, Object, Reflect};
    use worker::wasm_bindgen::{JsCast, JsValue};
    if tz.trim().is_empty() {
        return None;
    }
    let ms = iso_to_epoch_ms(iso)?;
    let opts = Object::new();
    for (k, v) in [
        ("timeZone", tz.trim()),
        ("hourCycle", "h23"),
        ("month", "2-digit"),
        ("day", "2-digit"),
        ("hour", "2-digit"),
        ("minute", "2-digit"),
    ] {
        Reflect::set(&opts, &JsValue::from_str(k), &JsValue::from_str(v)).ok()?;
    }
    let intl = Reflect::get(&worker::js_sys::global(), &JsValue::from_str("Intl")).ok()?;
    let ctor: Function = Reflect::get(&intl, &JsValue::from_str("DateTimeFormat")).ok()?.dyn_into().ok()?;
    let args = Array::of2(&JsValue::from_str("en-US"), &opts);
    // Reflect::construct returns Err on a RangeError (unknown zone) instead of trapping.
    let fmt = Reflect::construct(&ctor, &args).ok()?;
    let f: Function = Reflect::get(&fmt, &JsValue::from_str("formatToParts")).ok()?.dyn_into().ok()?;
    let parts = Reflect::apply(&f, &fmt, &Array::of1(&Date::new(&JsValue::from_f64(ms)))).ok()?;
    let parts: Array = parts.dyn_into().ok()?;
    let mut out = Vec::new();
    for p in parts.iter() {
        let t = Reflect::get(&p, &JsValue::from_str("type")).ok()?.as_string()?;
        let v = Reflect::get(&p, &JsValue::from_str("value")).ok()?.as_string()?;
        out.push((t, v));
    }
    stamp_from_parts(&out)
}

/// Host builds (tests, native check) have no `Intl`: no stamp.
#[cfg(not(target_arch = "wasm32"))]
pub fn format_in_tz(_iso: &str, _tz: &str) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_z_and_offset_to_epoch() {
        // 2026-11-14 23:39 UTC
        assert_eq!(iso_to_epoch_ms("2026-11-14T23:39:12.000Z"), Some(1_794_699_540_000.0));
        // same instant written with +09:00
        assert_eq!(iso_to_epoch_ms("2026-11-15T08:39:00+09:00"), Some(1_794_699_540_000.0));
        assert_eq!(iso_to_epoch_ms("1970-01-01T00:00:00Z"), Some(0.0));
    }

    #[test]
    fn rejects_garbage() {
        for bad in ["", "not-a-time", "2026-13-01T00:00:00Z", "2026-11-31T00:00:00Z", "2026-11-14T24:00:00Z", "2026-11-14T10:00:00"] {
            assert_eq!(iso_to_epoch_ms(bad), None, "{bad}");
        }
    }

    #[test]
    fn stamp_from_parts_assembles_or_omits() {
        let p = |v: &[(&str, &str)]| v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect::<Vec<_>>();
        let ok = p(&[("month", "11"), ("literal", "/"), ("day", "15"), ("hour", "08"), ("minute", "39")]);
        assert_eq!(stamp_from_parts(&ok).as_deref(), Some("11/15 08:39"));
        assert_eq!(stamp_from_parts(&p(&[("month", "11"), ("day", "15"), ("hour", "24"), ("minute", "05")])).as_deref(), Some("11/15 00:05"));
        assert_eq!(stamp_from_parts(&p(&[("month", "11"), ("day", "15")])), None);
    }

    #[test]
    fn host_has_no_intl_so_no_stamp() {
        assert_eq!(format_in_tz("2026-11-14T23:39:12.000Z", "Asia/Tokyo"), None);
    }
}
