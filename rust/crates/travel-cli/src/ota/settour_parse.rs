//! settour custom parser (char-by-char, NO regex) — Rust port of gwebcdb
//! `bridge/ota_parse.py::parse_settour`. settour's FIT pages don't fit the generic regex ruleset
//! (`has_custom_parser=1`), so this bespoke scanner reproduces the known-good Python output.
//! Oracle: `test_settour_oracle_full_offer` (capture settour-test-0620).

use crate::ota::regex_parse::ParsedOffer;
use travel_db::repo::parser_rules::ParserRuleRow;

fn digits_only(value: &str) -> String {
    value.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// Scan for `YYYY/MM/DD` runs (10 chars: 4 digits `/` 2 digits `/` 2 digits), dedup in order,
/// return (first, second) normalized to `YYYY-MM-DD`. Mirrors Python `s_extract_date_range`.
fn extract_date_range(text: &str) -> (String, String) {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut dates: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i + 10 <= n {
        let w = &chars[i..i + 10];
        let is_date = w[0].is_ascii_digit()
            && w[1].is_ascii_digit()
            && w[2].is_ascii_digit()
            && w[3].is_ascii_digit()
            && w[4] == '/'
            && w[5].is_ascii_digit()
            && w[6].is_ascii_digit()
            && w[7] == '/'
            && w[8].is_ascii_digit()
            && w[9].is_ascii_digit();
        if is_date {
            let s: String = w.iter().collect::<String>().replace('/', "-");
            if !dates.contains(&s) {
                dates.push(s);
            }
            i += 10;
        } else {
            i += 1;
        }
    }
    let depart = dates.first().cloned().unwrap_or_default();
    let ret = dates.get(1).cloned().unwrap_or_default();
    (depart, ret)
}

/// Digits between the first `start` marker and the next `end` marker after it.
/// Mirrors Python `s_capture_after`.
fn capture_after(text: &str, start: &str, end: &str) -> Option<String> {
    let si = text.find(start)?;
    let rest = &text[si + start.len()..];
    let ei = rest.find(end)?;
    let mid = digits_only(&rest[..ei]);
    if mid.is_empty() {
        None
    } else {
        Some(mid)
    }
}

/// Nights from `共N晚`, else `共N日` (days → nights = days-1). Mirrors Python `s_extract_nights`.
fn extract_nights(text: &str) -> Option<i64> {
    if let Some(n) = capture_after(text, "共", "晚") {
        return n.parse().ok();
    }
    if let Some(n) = capture_after(text, "共", "日") {
        let d: i64 = n.parse().ok()?;
        return Some(if d >= 1 { d - 1 } else { 0 });
    }
    None
}

/// First numeric run of >=4 digits (commas allowed inside a run). Mirrors Python
/// `s_extract_first_amount`.
fn extract_first_amount(text: &str) -> Option<i64> {
    let mut digits = String::new();
    let mut started = false;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
            started = true;
        } else if ch == ',' && started {
            continue;
        } else if started {
            if digits.len() >= 4 {
                return digits.parse().ok();
            }
            digits.clear();
            started = false;
        }
    }
    if digits.len() >= 4 {
        digits.parse().ok()
    } else {
        None
    }
}

/// Total price after the `機加酒未稅總價` marker. Mirrors Python `s_extract_settour_total`.
fn extract_total(text: &str) -> Option<i64> {
    let idx = text.find("機加酒未稅總價")?;
    extract_first_amount(&text[idx..])
}

/// Up to two flight codes (2 uppercase ASCII letters + >=2 digits, total 4..=6 chars), in order,
/// deduped. Mirrors Python `s_extract_flight_numbers`.
fn extract_flight_numbers(text: &str) -> (Option<String>, Option<String>) {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut found: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i + 2 < n {
        let is_code_start = chars[i].is_ascii_uppercase()
            && chars[i + 1].is_ascii_uppercase()
            && chars[i + 2].is_ascii_digit();
        if is_code_start {
            let mut code = String::new();
            code.push(chars[i]);
            code.push(chars[i + 1]);
            let mut j = i + 2;
            while j < n && chars[j].is_ascii_digit() {
                code.push(chars[j]);
                j += 1;
            }
            if (4..=6).contains(&code.len()) && !found.contains(&code) {
                found.push(code);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    (found.first().cloned(), found.get(1).cloned())
}

/// First non-empty line after a line that starts with `飯店` and contains `入住`. Mirrors Python
/// `s_extract_settour_hotel`.
fn extract_hotel(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.starts_with("飯店") && t.contains("入住") {
            for nxt in &lines[i + 1..] {
                let nn = nxt.trim();
                if !nn.is_empty() {
                    return Some(nn.to_string());
                }
            }
        }
    }
    None
}

/// Parse a settour FIT capture into a single offer. `rule` supplies the `product_type`/`currency`
/// (the catalog row) so the output keys identically to the generic path; price_per_person is the
/// settour 2-pax split `(total + 1) / 2`.
pub fn parse_settour(raw_text: &str, rule: &ParserRuleRow) -> Result<Vec<ParsedOffer>, String> {
    let (depart, ret) = extract_date_range(raw_text);
    let nights = extract_nights(raw_text);
    let total = extract_total(raw_text);
    let (fout, fret) = extract_flight_numbers(raw_text);
    let hotel = extract_hotel(raw_text);
    let per_person = match total {
        Some(t) => (t + 1) / 2, // settour = 2 pax
        None => 0,
    };
    Ok(vec![ParsedOffer {
        product_type: rule.product_type.clone(),
        departure_date: depart,
        return_date: ret,
        nights,
        price_per_person: per_person,
        currency: rule.currency.clone(),
        flight_outbound: fout,
        flight_return: fret,
        airline: None,
        hotel_name: hotel,
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settour_rule() -> ParserRuleRow {
        ParserRuleRow {
            source_id: "settour".to_string(),
            product_type: "fit".to_string(),
            date_range_rx: String::new(),
            nights_rx: String::new(),
            nights_is_days: false,
            price_marker: String::new(),
            price_amount_rx: String::new(),
            price_basis: "total".to_string(),
            pax_divisor: 2,
            flight_rx: String::new(),
            hotel_anchor_rx: String::new(),
            airline_rx: String::new(),
            hotel_name_rx: String::new(),
            currency: "TWD".to_string(),
            has_custom_parser: true,
        }
    }

    // Oracle text from gwebcdb test_ota_parse.py (capture settour-test-0620).
    const SETTOUR_TEXT: &str = concat!(
        "台灣桃園機場TPE大阪關西國際機場KIX ",
        "2026/06/20(六)~2026/06/24(三) 1間客房，2成人 ",
        "去程：2026/06/20 (六) 台灣虎航 IT212 15:15 TPE 直 19:00 KIX ",
        "回程：2026/06/24 (三) 台灣虎航 IT211 11:15 KIX 直 13:10 TPE ",
        "共4晚 機加酒未稅總價 36,587 元起\n",
        "飯店 京都 入住：2026/06/20\n微笑飯店京都烏丸五條\n房型 雙人房"
    );

    #[test]
    fn settour_oracle_full_offer() {
        let offers = parse_settour(SETTOUR_TEXT, &settour_rule()).unwrap();
        assert_eq!(offers.len(), 1);
        let o = &offers[0];
        assert_eq!(o.departure_date, "2026-06-20");
        assert_eq!(o.return_date, "2026-06-24");
        assert_eq!(o.nights, Some(4));
        assert_eq!(o.price_per_person, 18294); // (36587 + 1) / 2
        assert_eq!(o.flight_outbound.as_deref(), Some("IT212"));
        assert_eq!(o.flight_return.as_deref(), Some("IT211"));
        assert_eq!(o.hotel_name.as_deref(), Some("微笑飯店京都烏丸五條"));
        assert_eq!(o.product_type, "fit");
        assert_eq!(o.currency, "TWD");
        assert_eq!(o.airline, None);
    }

    #[test]
    fn first_amount_rejects_short_runs() {
        assert_eq!(extract_first_amount("abc 12 def 36,587 x"), Some(36587));
        assert_eq!(extract_first_amount("only 123 here"), None); // < 4 digits
    }

    #[test]
    fn nights_days_variant() {
        let offers = parse_settour(
            "2026/06/20~2026/06/24 共5日 機加酒未稅總價 30,000 飯店 X 入住：x\n旅館A",
            &settour_rule(),
        )
        .unwrap();
        assert_eq!(offers[0].nights, Some(4)); // 共5日 → 4 nights
    }

    #[test]
    fn date_range_dedups_in_order() {
        let (d, r) = extract_date_range("x 2026/06/20 y 2026/06/20 z 2026/06/24 w");
        assert_eq!(d, "2026-06-20");
        assert_eq!(r, "2026-06-24");
    }
}
