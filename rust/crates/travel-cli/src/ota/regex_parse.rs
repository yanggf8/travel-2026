use regex::Regex;
use travel_db::repo::parser_rules::ParserRuleRow;

#[derive(Debug, Clone)]
pub struct ParsedOffer {
    pub product_type: String,
    pub departure_date: String,
    pub return_date: String,
    pub nights: Option<i64>,
    pub price_per_person: i64,
    pub currency: String,
    pub flight_outbound: Option<String>,
    pub flight_return: Option<String>,
    pub airline: Option<String>,
    pub hotel_name: Option<String>,
}

fn digits_only(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_digit())
        .collect()
}

fn parse_amount(value: &str) -> Result<i64, String> {
    let d = digits_only(value);
    if d.is_empty() {
        Err(format!("invalid price amount {value:?}"))
    } else {
        d.parse().map_err(|e| format!("invalid price digits: {e}"))
    }
}

fn normalize_rule_date(value: &str) -> Result<String, String> {
    let s = value.replace('/', "-");
    if s.len() == 10
        && s.as_bytes().get(4) == Some(&b'-')
        && s.as_bytes().get(7) == Some(&b'-')
        && s[..4].chars().all(|c| c.is_ascii_digit())
        && s[5..7].chars().all(|c| c.is_ascii_digit())
        && s[8..10].chars().all(|c| c.is_ascii_digit())
    {
        Ok(s)
    } else {
        Err(format!("invalid date {value:?}; expected YYYY-MM-DD"))
    }
}

fn group1_or_0(caps: &regex::Captures) -> String {
    caps.get(1)
        .or_else(|| caps.get(0))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default()
}

fn extract_rule_date_range(raw_text: &str, rule: &ParserRuleRow) -> Result<(String, String), String> {
    let rx = Regex::new(&rule.date_range_rx)
        .map_err(|e| format!("invalid date_range_rx for {}: {e}", rule.source_id))?;
    let caps = rx
        .captures(raw_text)
        .ok_or_else(|| format!("rule '{}' date_range_rx found no match", rule.source_id))?;
    let depart = normalize_rule_date(
        caps.get(1)
            .ok_or_else(|| {
                format!(
                    "rule '{}' date_range_rx must capture departure in group 1",
                    rule.source_id
                )
            })?
            .as_str(),
    )?;
    let ret = if let Some(g2) = caps.get(2) {
        normalize_rule_date(g2.as_str())?
    } else if rule.product_type == "flight" {
        String::new()
    } else {
        return Err(format!(
            "rule '{}' date_range_rx must capture return date in group 2 for product_type={}",
            rule.source_id, rule.product_type
        ));
    };
    Ok((depart, ret))
}

fn extract_rule_nights(raw_text: &str, rule: &ParserRuleRow) -> Result<i64, String> {
    let rx = Regex::new(&rule.nights_rx)
        .map_err(|e| format!("invalid nights_rx for {}: {e}", rule.source_id))?;
    let caps = rx
        .captures(raw_text)
        .ok_or_else(|| format!("rule '{}' nights_rx found no match", rule.source_id))?;
    let group_text = if rule.nights_is_days {
        caps.get(1).map(|m| m.as_str())
    } else {
        // Python parity: m.lastindex — the highest-numbered CAPTURE group (>=1) that
        // participated in the match. Group 0 (the whole match) is excluded, so a regex whose
        // declared groups all failed to participate yields None and fails the parse (rather
        // than silently extracting nights from the entire matched text).
        (1..caps.len())
            .rev()
            .find_map(|i| caps.get(i))
            .map(|m| m.as_str())
    };
    let group_text = group_text.ok_or_else(|| {
        format!("rule '{}' nights_rx did not capture a group", rule.source_id)
    })?;
    let d = digits_only(group_text);
    if d.is_empty() {
        return Err(format!(
            "rule '{}' nights_rx captured no digits",
            rule.source_id
        ));
    }
    let value: i64 = d.parse().map_err(|e| format!("nights digits: {e}"))?;
    Ok(if rule.nights_is_days {
        if value >= 1 {
            value - 1
        } else {
            0
        }
    } else {
        value
    })
}

fn extract_rule_price(raw_text: &str, rule: &ParserRuleRow) -> Result<(Option<i64>, i64), String> {
    let idx = raw_text
        .find(&rule.price_marker)
        .ok_or_else(|| {
            format!(
                "rule '{}' could not find price_marker={:?}",
                rule.source_id, rule.price_marker
            )
        })?;
    let scope = &raw_text[idx + rule.price_marker.len()..];
    let rx = Regex::new(&rule.price_amount_rx)
        .map_err(|e| format!("invalid price_amount_rx for {}: {e}", rule.source_id))?;
    let caps = rx.captures(scope).ok_or_else(|| {
        format!(
            "rule '{}' could not extract price with price_amount_rx",
            rule.source_id
        )
    })?;
    let raw_amount = caps.get(1).ok_or_else(|| {
        format!(
            "rule '{}' price_amount_rx must capture amount in group 1",
            rule.source_id
        )
    })?;
    let amount = parse_amount(raw_amount.as_str())?;
    match rule.price_basis.as_str() {
        "total" => {
            if rule.pax_divisor <= 0 {
                return Err(format!(
                    "rule '{}' has invalid pax_divisor={}",
                    rule.source_id, rule.pax_divisor
                ));
            }
            let per_person = (amount + rule.pax_divisor - 1) / rule.pax_divisor;
            Ok((Some(amount), per_person))
        }
        "per_person" => {
            let total = if rule.pax_divisor > 0 {
                Some(amount * rule.pax_divisor)
            } else {
                None
            };
            Ok((total, amount))
        }
        other => Err(format!(
            "rule '{}' has unsupported price_basis={other:?}",
            rule.source_id
        )),
    }
}

fn extract_rule_flights(raw_text: &str, rule: &ParserRuleRow) -> Result<(Option<String>, Option<String>), String> {
    let rx = Regex::new(&rule.flight_rx)
        .map_err(|e| format!("invalid flight_rx for {}: {e}", rule.source_id))?;
    let mut found: Vec<String> = Vec::new();
    for caps in rx.captures_iter(raw_text) {
        let code = group1_or_0(&caps).replace(' ', "");
        if !code.is_empty() && !found.contains(&code) {
            found.push(code);
        }
    }
    Ok((
        found.first().cloned(),
        found.get(1).cloned(),
    ))
}

fn extract_rule_airline(raw_text: &str, rule: &ParserRuleRow) -> Result<Option<String>, String> {
    if rule.airline_rx.is_empty() {
        return Ok(None);
    }
    let rx = Regex::new(&rule.airline_rx)
        .map_err(|e| format!("invalid airline_rx for {}: {e}", rule.source_id))?;
    let caps = rx.captures(raw_text);
    Ok(caps.and_then(|c| {
        let v = group1_or_0(&c).trim().to_string();
        if v.is_empty() {
            None
        } else {
            Some(v)
        }
    }))
}

fn extract_rule_hotel(raw_text: &str, rule: &ParserRuleRow) -> Result<String, String> {
    if !rule.hotel_name_rx.is_empty() {
        let rx = Regex::new(&rule.hotel_name_rx)
            .map_err(|e| format!("invalid hotel_name_rx for {}: {e}", rule.source_id))?;
        let caps = rx.captures(raw_text).ok_or_else(|| {
            format!("rule '{}' hotel_name_rx found no hotel", rule.source_id)
        })?;
        let value = group1_or_0(&caps).trim().to_string();
        if value.is_empty() {
            Err(format!("rule '{}' hotel_name_rx found no hotel", rule.source_id))
        } else {
            Ok(value)
        }
    } else {
        let anchor = Regex::new(&rule.hotel_anchor_rx)
            .map_err(|e| format!("invalid hotel_anchor_rx for {}: {e}", rule.source_id))?;
        let lines: Vec<&str> = raw_text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if anchor.is_match(line) {
                for nxt in &lines[i + 1..] {
                    let n = nxt.trim();
                    if !n.is_empty() {
                        return Ok(n.to_string());
                    }
                }
            }
        }
        Err(format!(
            "rule '{}' could not find a hotel line after anchor",
            rule.source_id
        ))
    }
}

/// Generic regex parser (Python `parse_generic` parity).
pub fn parse_generic(raw_text: &str, rule: &ParserRuleRow) -> Result<Vec<ParsedOffer>, String> {
    let (depart, ret) = extract_rule_date_range(raw_text, rule)?;
    let nights = if rule.product_type == "flight" {
        None
    } else {
        Some(extract_rule_nights(raw_text, rule)?)
    };
    let (_price_total, per_person) = extract_rule_price(raw_text, rule)?;
    let flights = extract_rule_flights(raw_text, rule)?;
    let airline = extract_rule_airline(raw_text, rule)?;
    let hotel = if rule.product_type == "flight" {
        None
    } else {
        Some(extract_rule_hotel(raw_text, rule)?)
    };

    if rule.product_type == "flight" && flights.0.is_none() && airline.is_none() {
        return Err(format!(
            "rule '{}' product_type=flight requires a flight number from flight_rx or airline from airline_rx",
            rule.source_id
        ));
    }

    Ok(vec![ParsedOffer {
        product_type: rule.product_type.clone(),
        departure_date: depart,
        return_date: ret,
        nights,
        price_per_person: per_person,
        currency: rule.currency.clone(),
        flight_outbound: flights.0,
        flight_return: flights.1,
        airline,
        hotel_name: hotel,
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule_with_nights_rx(rx: &str) -> ParserRuleRow {
        ParserRuleRow {
            source_id: "zztest".to_string(),
            product_type: "package".to_string(),
            date_range_rx: String::new(),
            nights_rx: rx.to_string(),
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
            has_custom_parser: false,
        }
    }

    #[test]
    fn nights_uses_last_participating_capture_group() {
        // Two capture groups; only group 2 participates (the digits). lastindex == 2.
        let rule = rule_with_nights_rx(r"(\d+)?日(\d+)夜");
        let n = extract_rule_nights("行程5日4夜", &rule).unwrap();
        assert_eq!(n, 4);
    }

    #[test]
    fn nights_fails_when_no_capture_group_participates() {
        // Declared alternation groups, but the match comes from neither — Python raises
        // "did not capture a group"; we must NOT fall back to group 0 (the whole match).
        let rule = rule_with_nights_rx(r"(?:(早鳥)|(晚鳥))?共\d+晚");
        let err = extract_rule_nights("共7晚", &rule).unwrap_err();
        assert!(err.contains("did not capture a group"), "got: {err}");
    }
}