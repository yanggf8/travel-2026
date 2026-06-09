// Scrape File Parser — Rust port of src/scrapers/scrape-file-parser.ts
//
// Parses scraper JSON output files into CanonicalOffer structs.
// Supports two formats:
//   Format A: `{ "offers": [...] }` — scraper listings output
//   Format B: `{ "url", "extracted", ... }` — raw single scrape
//
// Groups parsed offers by source_id (one ParsedOfferFile per source).
//
// PARITY NOTE: This must produce byte-identical output to the TS parser.
// `stable_hash8` uses SHA-1 first-8-hex-chars — identical to the TS
// `crypto.createHash('sha1').update(input).digest('hex').slice(0, 8)`.

use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

// ============================================================================
// Public types
// ============================================================================

/// Parsed offers from one or more files, grouped by source.
#[derive(Debug)]
pub struct ParsedOfferFile {
    pub source_id: String,
    pub file_path: String,
    pub file_name: String,
    pub offers: Vec<CanonicalOffer>,
    pub warnings: Vec<String>,
}

/// Canonical offer — mirrors TS CanonicalOffer interface.
#[derive(Debug, Clone)]
pub struct CanonicalOffer {
    pub id: String,
    pub source_id: String,
    pub offer_type: String, // "package" | "flight" | "hotel"
    pub title: String,
    pub url: String,
    pub currency: String,
    pub price_per_person: i64,
    pub price_total: Option<i64>,
    pub availability: String,
    pub flight: Option<OfferFlight>,
    pub hotel: Option<OfferHotel>,
    pub includes: Vec<String>,
    pub date_pricing: Vec<DatePricingEntry>,
    pub best_value: Option<BestValue>,
    pub scraped_at: String,
}

#[derive(Debug, Clone)]
pub struct OfferFlight {
    pub outbound: FlightSegment,
    pub return_leg: FlightSegment,
}

#[derive(Debug, Clone)]
// Some fields (airports, terminals, date) are parsed for fidelity to the
// canonical offer but are NOT persisted: the TS `plan_offer_flights` flush
// (plan-repository.ts syncNormalizedTables) only writes flight_number, airline,
// airline_code, departure/arrival code + time. Keeping the fields documents the
// full parsed shape and matches mapFlightLeg; `dead_code` is therefore expected.
#[allow(dead_code)]
pub struct FlightSegment {
    pub flight_number: String,
    pub airline: String,
    pub airline_code: Option<String>,
    pub departure_airport: String,
    pub departure_code: String,
    pub departure_terminal: Option<String>,
    pub departure_time: String,
    pub arrival_airport: String,
    pub arrival_code: String,
    pub arrival_terminal: Option<String>,
    pub arrival_time: String,
    pub date: String,
}

#[derive(Debug, Clone)]
// `room_type` is parsed (mapHotel sets it) but not persisted — the TS
// `plan_offer_hotels` flush writes only name, slug, area, star_rating (+ access
// child rows). Expected dead_code.
#[allow(dead_code)]
pub struct OfferHotel {
    pub name: String,
    pub slug: Option<String>,
    pub area: String,
    pub star_rating: Option<i64>,
    pub access: Vec<String>,
    pub room_type: Option<String>,
}

#[derive(Debug, Clone)]
// `price_total` / `notes` are parsed but not persisted — the TS
// `plan_offer_date_pricing` flush writes only date, price, availability,
// seats_remaining. Expected dead_code.
#[allow(dead_code)]
pub struct DatePricingEntry {
    pub date: String,
    pub price_per_person: i64,
    pub price_total: Option<i64>,
    pub availability: String,
    pub seats_remaining: Option<i64>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone)]
// `price_total` is parsed (mapBestValue derives it) but not persisted — the TS
// `plan_offer_best_value` flush writes best_date, best_price, currency only.
#[allow(dead_code)]
pub struct BestValue {
    pub date: String,
    pub price_per_person: i64,
    pub price_total: i64,
}

/// Parse options — mirrors the TS opts parameter.
// `pax` only feeds the (unpersisted) bestValue.price_total computation, so it is
// carried for parity with the TS opts but is not otherwise read.
#[allow(dead_code)]
pub struct ParseOpts {
    pub start: Option<String>,
    pub end: Option<String>,
    pub include_undated: bool,
    pub pax: i64,
}

impl Default for ParseOpts {
    fn default() -> Self {
        Self {
            start: None,
            end: None,
            include_undated: true,
            pax: 2,
        }
    }
}

// ============================================================================
// Public API
// ============================================================================

/// Parse scrape JSON files into per-source offer groups.
/// Mirrors TS `parseScrapeFiles()`.
pub fn parse_scrape_files(file_paths: &[String], opts: &ParseOpts) -> Vec<ParsedOfferFile> {
    // Type-erased JSON value
    type JsonVal = serde_json::Value;

    // Per-source accumulator
    struct SourceGroup {
        offers: Vec<CanonicalOffer>,
        warnings: Vec<String>,
        file_path: String,
        file_name: String,
    }

    let mut by_source: HashMap<String, SourceGroup> = HashMap::new();

    for fp in file_paths {
        let file_name = Path::new(fp)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        let raw_str = match fs::read_to_string(fp) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let raw: JsonVal = match serde_json::from_str(&raw_str) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // TS: `if (!raw || typeof raw !== 'object' || Array.isArray(raw)) continue;`
        if !raw.is_object() {
            continue;
        }
        let obj = raw.as_object().unwrap();

        let mut parsed_offers: Vec<CanonicalOffer> = Vec::new();
        let mut file_warnings: Vec<String> = Vec::new();

        if let Some(offers_arr) = obj.get("offers").and_then(|v| v.as_array()) {
            // Format A — `{ offers: [...] }`
            for o in offers_arr {
                if !o.is_object() {
                    continue;
                }
                match parse_format_a_offer(o.as_object().unwrap(), &file_name) {
                    Ok(Some(offer)) => parsed_offers.push(offer),
                    Err(e) => file_warnings
                        .push(format!("Parse error in {file_name}: {e}")),
                    _ => {}
                }
            }
        } else {
            // Format B — raw single scrape with `extracted` key
            match parse_format_b_offer(obj, &file_name) {
                Ok(Some(offer)) => parsed_offers.push(offer),
                Err(e) => file_warnings
                    .push(format!("Parse error in {file_name}: {e}")),
                _ => {}
            }
        }

        // Apply date filter
        let filtered: Vec<CanonicalOffer> = parsed_offers
            .into_iter()
            .filter(|o| {
                let date = get_representative_date(o);
                is_within_date_range(
                    date.as_deref(),
                    opts.start.as_deref(),
                    opts.end.as_deref(),
                    opts.include_undated,
                )
            })
            .collect();

        // Group by source_id
        for offer in filtered {
            let sid = offer.source_id.clone();
            by_source
                .entry(sid.clone())
                .or_insert_with(|| SourceGroup {
                    offers: Vec::new(),
                    warnings: Vec::new(),
                    file_path: fp.clone(),
                    file_name: file_name.clone(),
                })
                .offers
                .push(offer);
        }

        // Attach file-level warnings to every group that got offers from this file
        // (mirrors TS: pushes to all groups)
        if !file_warnings.is_empty() {
            for group in by_source.values_mut() {
                group.warnings.extend(file_warnings.iter().cloned());
            }
        }
    }

    // Deduplicate warnings per group (mirrors TS: `[...new Set(group.warnings)]`)
    by_source
        .into_iter()
        .map(|(source_id, group)| {
            let mut seen = std::collections::HashSet::new();
            let deduped: Vec<String> = group
                .warnings
                .into_iter()
                .filter(|w| seen.insert(w.clone()))
                .collect();
            ParsedOfferFile {
                source_id,
                file_path: group.file_path,
                file_name: group.file_name,
                offers: group.offers,
                warnings: deduped,
            }
        })
        .collect()
}

// ============================================================================
// Inference helpers
// ============================================================================

/// Mirrors TS `inferSourceIdFromFilename`.
pub fn infer_source_id_from_filename(filename: &str) -> &'static str {
    let f = filename.to_ascii_lowercase();
    if f.contains("besttour") {
        return "besttour";
    }
    if f.contains("liontravel") {
        return "liontravel";
    }
    if f.contains("lifetour") {
        return "lifetour";
    }
    if f.contains("settour") {
        return "settour";
    }
    if f.contains("travel4u") {
        return "travel4u";
    }
    if f.contains("eztravel") {
        return "eztravel";
    }
    if f.contains("tigerair") {
        return "tigerair";
    }
    if f.contains("agoda") {
        return "agoda";
    }
    if f.contains("booking") {
        return "booking";
    }
    "unknown"
}

/// Mirrors TS `inferDestinationFromFilename`.
#[allow(dead_code)]
pub fn infer_destination_from_filename(filename: &str) -> Option<&'static str> {
    let f = filename.to_ascii_lowercase();
    if f.contains("tokyo") || f.contains("tyo") {
        return Some("tokyo_2026");
    }
    if f.contains("osaka_kyoto") {
        return Some("osaka_kyoto_2026");
    }
    if f.contains("osaka") || f.contains("kansai") || f.contains("kyoto") || f.contains("kix") {
        return Some("osaka_2026");
    }
    None
}

// ============================================================================
// Internal helpers
// ============================================================================

fn infer_source_id_from_url(url: &str) -> &'static str {
    let u = url.to_ascii_lowercase();
    if u.contains("besttour.com.tw") {
        return "besttour";
    }
    if u.contains("liontravel.com") {
        return "liontravel";
    }
    if u.contains("lifetour.com.tw") {
        return "lifetour";
    }
    if u.contains("settour.com.tw") {
        return "settour";
    }
    if u.contains("travel4u.com.tw") {
        return "travel4u";
    }
    if u.contains("eztravel") {
        return "eztravel";
    }
    if u.contains("tigerair") {
        return "tigerair";
    }
    if u.contains("agoda") {
        return "agoda";
    }
    if u.contains("booking.com") {
        return "booking";
    }
    "unknown"
}

/// SHA-1 first 8 hex chars — identical to TS `crypto.createHash('sha1').update(input).digest('hex').slice(0, 8)`.
fn stable_hash_8(input: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();
    format!("{:x}", result)
        .get(..8)
        .unwrap_or(&format!("{:x}", result))
        .to_string()
}

fn normalize_iso_date(val: &serde_json::Value) -> Option<String> {
    let s = val.as_str()?.trim();
    if s.is_empty() {
        return None;
    }
    // YYYY-MM-DD
    if s.len() == 10
        && s.as_bytes()[4] == b'-'
        && s.as_bytes()[7] == b'-'
    {
        return Some(s.to_string());
    }
    // YYYY/M/D → YYYY-MM-DD
    let re = regex::Regex::new(r"^(\d{4})/(\d{1,2})/(\d{1,2})$").unwrap();
    if let Some(caps) = re.captures(s) {
        let y = &caps[1];
        let m = format!("{:0>2}", &caps[2]);
        let d = format!("{:0>2}", &caps[3]);
        return Some(format!("{y}-{m}-{d}"));
    }
    None
}

fn product_code_from_url(url: &str) -> Option<String> {
    if url.is_empty() {
        return None;
    }
    // Try URL parsing
    if let Ok(parsed) = url::Url::parse(url) {
        let segs: Vec<&str> = parsed.path_segments().map(|s| s.collect()).unwrap_or_default();
        let last = segs.last()?;
        if last.is_empty() {
            return None;
        }
        // Strip .html/.htm suffix
        let code = regex::Regex::new(r"(?i)\.html?$")
            .unwrap()
            .replace(last, "")
            .to_string();
        if code.is_empty() {
            return None;
        }
        return Some(code);
    }
    // Fallback: split on / and ? manually
    let base = url.split('?').next().unwrap_or("");
    let parts: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
    parts.last().map(|s| s.to_string())
}

fn map_flight_leg(
    leg: &serde_json::Value,
) -> Option<FlightSegment> {
    let obj = leg.as_object()?;
    let flight_number = get_str_ref(obj, "flight_number")
        .or_else(|| get_str_ref(obj, "flightNumber"))
        .unwrap_or_default();
    if flight_number.is_empty() {
        return None;
    }
    Some(FlightSegment {
        flight_number: flight_number.to_string(),
        airline: get_str_ref(obj, "airline").unwrap_or_default().to_string(),
        // TS mapFlightLeg: `typeof leg.airline_code === 'string' ? leg.airline_code : undefined`
        // — KEEPS an empty string ("") rather than dropping it to None. The DB flush then
        // writes '' (sqlText("") => '') not NULL. Use the keep-empty getter to match.
        airline_code: get_str_keep_empty(obj, "airline_code")
            .or_else(|| get_str_keep_empty(obj, "airlineCode")),
        departure_airport: String::new(),
        departure_code: get_str_ref(obj, "departure_code")
            .or_else(|| get_str_ref(obj, "departure_airport_code"))
            .or_else(|| get_str_ref(obj, "departureCode"))
            .unwrap_or_default()
            .to_string(),
        departure_terminal: get_str_owned(obj, "departure_terminal"),
        departure_time: get_str_ref(obj, "departure_time")
            .or_else(|| get_str_ref(obj, "departureTime"))
            .unwrap_or_default()
            .to_string(),
        arrival_airport: String::new(),
        arrival_code: get_str_ref(obj, "arrival_code")
            .or_else(|| get_str_ref(obj, "arrival_airport_code"))
            .or_else(|| get_str_ref(obj, "arrivalCode"))
            .unwrap_or_default()
            .to_string(),
        arrival_terminal: get_str_owned(obj, "arrival_terminal"),
        arrival_time: get_str_ref(obj, "arrival_time")
            .or_else(|| get_str_ref(obj, "arrivalTime"))
            .unwrap_or_default()
            .to_string(),
        date: normalize_iso_date(obj.get("date").unwrap_or(&serde_json::Value::Null))
            .unwrap_or_default(),
    })
}

fn map_hotel(hotel: &serde_json::Value) -> Option<OfferHotel> {
    let obj = hotel.as_object()?;
    let name = get_str_ref(obj, "name").unwrap_or_default();
    let name_from_names = obj
        .get("names")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let effective_name = if !name.is_empty() {
        name
    } else {
        name_from_names
    };
    if effective_name.is_empty() {
        return None;
    }
    Some(OfferHotel {
        name: effective_name.to_string(),
        slug: get_str_owned(obj, "slug"),
        area: get_str_ref(obj, "area").unwrap_or_default().to_string(),
        star_rating: obj.get("star_rating").and_then(|v| v.as_i64()),
        access: obj
            .get("access")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default(),
        room_type: get_str_owned(obj, "room_type"),
    })
}

fn map_date_pricing_dict(
    dict: &serde_json::Map<String, serde_json::Value>,
) -> Vec<DatePricingEntry> {
    let mut result = Vec::new();
    for (date_key, entry) in dict {
        let normalized_date = match normalize_iso_date(&serde_json::Value::String(
            date_key.clone(),
        )) {
            Some(d) => d,
            None => continue,
        };
        let e = match entry.as_object() {
            Some(o) => o,
            None => continue,
        };
        let price = e
            .get("price")
            .and_then(|v| v.as_f64())
            .or_else(|| e.get("price_per_person").and_then(|v| v.as_f64()));
        let price = match price {
            Some(p) if p.is_finite() => p as i64,
            _ => continue,
        };
        result.push(DatePricingEntry {
            date: normalized_date,
            price_per_person: price,
            price_total: e.get("price_total").and_then(|v| v.as_i64()),
            availability: get_str_ref(e, "availability").unwrap_or("unknown").to_string(),
            seats_remaining: e.get("seats_remaining").and_then(|v| v.as_i64()),
            notes: get_str_owned(e, "notes"),
        });
    }
    result
}

fn map_date_pricing_array(arr: &[serde_json::Value]) -> Vec<DatePricingEntry> {
    arr.iter()
        .filter_map(|v| {
            let d = v.as_object()?;
            let date_raw = d.get("date").unwrap_or(&serde_json::Value::Null);
            let date = normalize_iso_date(date_raw)
                .or_else(|| date_raw.as_str().map(String::from))
                .unwrap_or_default();
            if date.is_empty() {
                return None;
            }
            let ppp = d
                .get("pricePerPerson")
                .and_then(|v| v.as_f64())
                .or_else(|| d.get("price_per_person").and_then(|v| v.as_f64()))
                .or_else(|| d.get("price").and_then(|v| v.as_f64()))
                .unwrap_or(0.0);
            if !ppp.is_finite() {
                return None;
            }
            Some(DatePricingEntry {
                date,
                price_per_person: ppp as i64,
                price_total: None,
                availability: d
                    .get("availability")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                seats_remaining: None,
                notes: None,
            })
        })
        .collect()
}

fn map_best_value(bv: &serde_json::Value) -> Option<BestValue> {
    let obj = bv.as_object()?;
    let date = normalize_iso_date(obj.get("date").unwrap_or(&serde_json::Value::Null))?;
    let price = obj
        .get("price_per_person")
        .and_then(|v| v.as_f64())
        .or_else(|| obj.get("pricePerPerson").and_then(|v| v.as_f64()))?;
    if !price.is_finite() {
        return None;
    }
    let total = obj
        .get("price_total")
        .and_then(|v| v.as_f64())
        .or_else(|| obj.get("priceTotal").and_then(|v| v.as_f64()))
        .unwrap_or(0.0) as i64;
    Some(BestValue {
        date,
        price_per_person: price as i64,
        price_total: total,
    })
}

fn get_representative_date(offer: &CanonicalOffer) -> Option<String> {
    if let Some(ref bv) = offer.best_value && !bv.date.is_empty() {
        return Some(bv.date.clone());
    }
    if !offer.date_pricing.is_empty() {
        let mut sorted: Vec<&DatePricingEntry> = offer.date_pricing.iter().collect();
        sorted.sort_by(|a, b| a.date.cmp(&b.date));
        return sorted.first().map(|e| e.date.clone());
    }
    None
}

fn is_within_date_range(
    date: Option<&str>,
    start: Option<&str>,
    end: Option<&str>,
    include_undated: bool,
) -> bool {
    if start.is_none() && end.is_none() {
        return true;
    }
    let date = match date {
        Some(d) => d,
        None => return include_undated,
    };
    if let Some(s) = start && date < s {
        return false;
    }
    if let Some(e) = end && date > e {
        return false;
    }
    true
}

// ============================================================================
// Format A parser — `{ offers: [...] }`
// ============================================================================

fn parse_format_a_offer(
    offer: &serde_json::Map<String, serde_json::Value>,
    file_name: &str,
) -> Result<Option<CanonicalOffer>, String> {
    let url = get_str_ref(offer, "url").unwrap_or_default().to_string();
    let source_id = get_str_owned(offer, "source_id")
        .or_else(|| get_str_owned(offer, "sourceId"))
        .or_else(|| {
            if !url.is_empty() {
                Some(infer_source_id_from_url(&url).to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| infer_source_id_from_filename(file_name).to_string());

    if source_id.is_empty() || source_id == "unknown" {
        return Ok(None);
    }

    let explicit_id = get_str_owned(offer, "id");
    let product_code = get_str_owned(offer, "product_code");
    let url_code = product_code_from_url(&url);
    let url_hash = stable_hash_8(&url);
    let id = explicit_id.unwrap_or_else(|| {
        if !url.is_empty() {
            let code = product_code
                .as_deref()
                .or(url_code.as_deref())
                .unwrap_or(&url_hash);
            format!("{}_{}", source_id, code)
        } else {
            String::new()
        }
    });
    if id.is_empty() {
        return Ok(None);
    }

    let type_raw = get_str_ref(offer, "type").unwrap_or("package").to_ascii_lowercase();
    let offer_type = match type_raw.as_str() {
        "flight" => "flight",
        "hotel" => "hotel",
        _ => "package",
    };

    let title = get_str_owned(offer, "title")
        .or_else(|| get_str_owned(offer, "name"))
        .unwrap_or_default();

    let currency = get_str_owned(offer, "currency").unwrap_or_else(|| "TWD".to_string());

    let ppp = offer
        .get("price_per_person")
        .and_then(|v| v.as_f64())
        .filter(|f| f.is_finite());
    let ppp2 = offer
        .get("pricePerPerson")
        .and_then(|v| v.as_f64())
        .filter(|f| f.is_finite());
    let price_per_person = match ppp.or(ppp2) {
        Some(p) => p as i64,
        None => return Ok(None),
    };

    let availability = get_str_owned(offer, "availability").unwrap_or_else(|| "unknown".to_string());

    // Flight
    let flight = offer
        .get("flight")
        .and_then(|v| v.as_object())
        .and_then(|f| {
            let outbound = map_flight_leg(f.get("outbound").unwrap_or(&serde_json::Value::Null));
            let ret = map_flight_leg(f.get("return").unwrap_or(&serde_json::Value::Null));
            match (outbound, ret) {
                (Some(o), Some(r)) => Some(OfferFlight {
                    outbound: o,
                    return_leg: r,
                }),
                _ => None,
            }
        });

    // Hotel
    let hotel = offer.get("hotel").and_then(map_hotel);

    // Includes
    let includes: Vec<String> = offer
        .get("includes")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    // Date pricing
    let date_pricing = if let Some(dp_raw) = offer
        .get("date_pricing")
        .or(offer.get("datePricing"))
    {
        if let Some(dp_obj) = dp_raw.as_object() {
            map_date_pricing_dict(dp_obj)
        } else if let Some(dp_arr) = dp_raw.as_array() {
            map_date_pricing_array(dp_arr)
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    // Best value
    let best_value = offer
        .get("best_value")
        .or(offer.get("bestValue"))
        .and_then(map_best_value);

    let scraped_at = get_str_owned(offer, "scraped_at")
        .or_else(|| get_str_owned(offer, "scrapedAt"))
        .unwrap_or_else(|| {
            // TS: `new Date().toISOString()` — approximate with current time
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        });

    Ok(Some(CanonicalOffer {
        id,
        source_id,
        offer_type: offer_type.to_string(),
        title,
        url,
        currency,
        price_per_person,
        price_total: offer.get("price_total").and_then(|v| v.as_i64()),
        availability,
        flight,
        hotel,
        includes,
        date_pricing,
        best_value,
        scraped_at,
    }))
}

// ============================================================================
// Format B parser — `{ url, extracted, ... }`
// ============================================================================

fn parse_format_b_offer(
    data: &serde_json::Map<String, serde_json::Value>,
    file_name: &str,
) -> Result<Option<CanonicalOffer>, String> {
    let url = get_str_ref(data, "url").unwrap_or_default().to_string();
    let extracted = data
        .get("extracted")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let source_id_from_data = get_str_owned(&extracted, "source_id")
        .or_else(|| get_str_owned(data, "source_id"))
        .or_else(|| get_str_owned(data, "sourceId"));
    let source_id = source_id_from_data
        .or_else(|| {
            if !url.is_empty() {
                Some(infer_source_id_from_url(&url).to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| infer_source_id_from_filename(file_name).to_string());

    if source_id.is_empty() || source_id == "unknown" {
        return Ok(None);
    }

    let id = format!(
        "{}_{}",
        source_id,
        product_code_from_url(&url)
            .as_deref()
            .unwrap_or(&stable_hash_8(&url))
    );

    // Price
    let price_raw = extracted.get("price").and_then(|v| v.as_object());
    let price_per_person = price_raw
        .and_then(|p| p.get("per_person"))
        .and_then(|v| v.as_f64())
        .filter(|f| f.is_finite());
    let price_per_person = match price_per_person {
        Some(p) => p as i64,
        None => return Ok(None),
    };

    let currency = price_raw
        .and_then(|p| get_str_owned(p, "currency"))
        .unwrap_or_else(|| "TWD".to_string());

    // Type inference
    let flight_raw = extracted.get("flight").and_then(|v| v.as_object());
    let hotel_raw = extracted.get("hotel").and_then(|v| v.as_object());
    let has_flight = flight_raw
        .map(|f| f.contains_key("outbound") || f.contains_key("return"))
        .unwrap_or(false);
    let has_hotel = hotel_raw
        .map(|h| h.contains_key("name") || h.contains_key("names"))
        .unwrap_or(false);
    let offer_type = match (has_flight, has_hotel) {
        (true, false) => "flight",
        (false, true) => "hotel",
        _ => "package",
    };

    // Flight
    let flight = flight_raw.and_then(|f| {
        let outbound = map_flight_leg(f.get("outbound").unwrap_or(&serde_json::Value::Null));
        let ret = map_flight_leg(f.get("return").unwrap_or(&serde_json::Value::Null));
        match (outbound, ret) {
            (Some(o), Some(r)) => Some(OfferFlight {
                outbound: o,
                return_leg: r,
            }),
            _ => None,
        }
    });

    // Hotel
    let hotel = extracted
        .get("hotel")
        .and_then(map_hotel);

    // Date pricing
    let date_pricing = extracted
        .get("date_pricing")
        .and_then(|v| v.as_object())
        .map(map_date_pricing_dict)
        .unwrap_or_default();

    let title = get_str_owned(data, "title").unwrap_or_default();

    let scraped_at = get_str_owned(data, "scraped_at")
        .or_else(|| get_str_owned(data, "scrapedAt"))
        .unwrap_or_else(|| {
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        });

    Ok(Some(CanonicalOffer {
        id,
        source_id,
        offer_type: offer_type.to_string(),
        title,
        url,
        currency,
        price_per_person,
        price_total: None,
        availability: "unknown".to_string(),
        flight,
        hotel,
        includes: Vec::new(),
        date_pricing,
        best_value: None,
        scraped_at,
    }))
}

// ============================================================================
// JSON value helpers
// ============================================================================

/// Get a non-empty string value as owned String.
fn get_str_owned(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    obj.get(key).and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string())
}

/// Like `get_str_owned` but KEEPS empty strings (returns `Some("")` when the key
/// is present and is a JSON string, even if empty). Mirrors the TS
/// `typeof x === 'string' ? x : undefined` pattern, where "" is still a string.
fn get_str_keep_empty(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    obj.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

/// Get any string value (including empty) as Option<&str> for comparisons.
fn get_str_ref<'a>(obj: &'a serde_json::Map<String, serde_json::Value>, key: &str) -> Option<&'a str> {
    obj.get(key).and_then(|v| v.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stable_hash_8() {
        // Must match TS: crypto.createHash('sha1').update("test").digest('hex').slice(0,8)
        let result = stable_hash_8("test");
        assert_eq!(result.len(), 8, "hash must be 8 hex chars: got {result}");
        // SHA1("test") = a94a8fe5ccb19ba61c4c0873d391e987982fbbd3
        assert_eq!(result, "a94a8fe5");
    }

    #[test]
    fn test_infer_source_id_from_filename() {
        assert_eq!(infer_source_id_from_filename("besttour-kansai-3n.json"), "besttour");
        assert_eq!(infer_source_id_from_filename("liontravel-detail.json"), "liontravel");
        assert_eq!(infer_source_id_from_filename("unknown-file.json"), "unknown");
    }

    #[test]
    fn test_infer_source_id_from_url() {
        assert_eq!(
            infer_source_id_from_url("https://www.besttour.com.tw/itinerary/ABC"),
            "besttour"
        );
        assert_eq!(
            infer_source_id_from_url("https://www.liontravel.com/product/123"),
            "liontravel"
        );
    }

    #[test]
    fn test_normalize_iso_date() {
        let v = serde_json::Value::String("2026-06-13".to_string());
        assert_eq!(normalize_iso_date(&v), Some("2026-06-13".to_string()));
        let v = serde_json::Value::String("2026/6/3".to_string());
        assert_eq!(normalize_iso_date(&v), Some("2026-06-03".to_string()));
        let v = serde_json::Value::String("".to_string());
        assert_eq!(normalize_iso_date(&v), None);
        let v = serde_json::Value::Null;
        assert_eq!(normalize_iso_date(&v), None);
    }

    #[test]
    fn test_product_code_from_url() {
        assert_eq!(
            product_code_from_url("https://www.besttour.com.tw/itinerary/UKB05BR260622U"),
            Some("UKB05BR260622U".to_string())
        );
        assert_eq!(
            product_code_from_url("https://example.com/foo/bar.html"),
            Some("bar".to_string())
        );
        assert_eq!(product_code_from_url(""), None);
    }

    #[test]
    fn test_is_within_date_range() {
        assert!(is_within_date_range(Some("2026-06-15"), None, None, true));
        assert!(is_within_date_range(
            Some("2026-06-15"),
            Some("2026-06-10"),
            Some("2026-06-20"),
            true
        ));
        assert!(!is_within_date_range(
            Some("2026-06-05"),
            Some("2026-06-10"),
            None,
            true
        ));
        assert!(!is_within_date_range(
            Some("2026-06-25"),
            None,
            Some("2026-06-20"),
            true
        ));
        assert!(is_within_date_range(None, Some("2026-06-10"), None, true));
        assert!(!is_within_date_range(None, Some("2026-06-10"), None, false));
    }

    #[test]
    fn test_parse_format_a_basic() {
        let json = r#"{
            "offers": [{
                "source_id": "besttour",
                "id": "test_123",
                "url": "https://www.besttour.com.tw/itinerary/ABC",
                "title": "Test Package",
                "price_per_person": 29900,
                "currency": "TWD",
                "availability": "available",
                "type": "package",
                "scraped_at": "2026-06-09T00:00:00Z"
            }]
        }"#;
        let tmp = std::env::temp_dir().join("test_parse_a.json");
        std::fs::write(&tmp, json).unwrap();
        let opts = ParseOpts::default();
        let results = parse_scrape_files(&[tmp.to_string_lossy().to_string()], &opts);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source_id, "besttour");
        assert_eq!(results[0].offers.len(), 1);
        assert_eq!(results[0].offers[0].id, "test_123");
        assert_eq!(results[0].offers[0].price_per_person, 29900);
    }

    #[test]
    fn test_parse_format_b_basic() {
        let json = r#"{
            "url": "https://www.besttour.com.tw/itinerary/UKB05BR260622U",
            "source_id": "besttour",
            "scraped_at": "2026-06-09T00:00:00Z",
            "extracted": {
                "price": { "per_person": 31888, "currency": "TWD" },
                "flight": {
                    "outbound": { "flight_number": "BR178", "departure_code": "TPE", "departure_time": "06:30", "arrival_code": "KIX", "arrival_time": "10:10", "date": "2026/06/13" },
                    "return": { "flight_number": "BR177", "departure_code": "KIX", "departure_time": "11:10", "arrival_code": "TPE", "arrival_time": "13:05", "date": "2026/06/17" }
                },
                "hotel": { "name": "Test Hotel", "area": "Shinjuku" }
            }
        }"#;
        let tmp = std::env::temp_dir().join("test_parse_b.json");
        std::fs::write(&tmp, json).unwrap();
        let opts = ParseOpts::default();
        let results = parse_scrape_files(&[tmp.to_string_lossy().to_string()], &opts);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source_id, "besttour");
        assert_eq!(results[0].offers.len(), 1);
        // ID = source_id + "_" + productCodeFromUrl
        assert_eq!(results[0].offers[0].id, "besttour_UKB05BR260622U");
        assert_eq!(results[0].offers[0].price_per_person, 31888);
        assert!(results[0].offers[0].flight.is_some());
        assert!(results[0].offers[0].hotel.is_some());
    }

    #[test]
    fn test_parse_skips_no_price() {
        // Format B with no price.per_person → returns null
        let json = r#"{
            "url": "https://www.besttour.com.tw/itinerary/ABC",
            "source_id": "besttour",
            "extracted": { "price": { "currency": "TWD" } }
        }"#;
        let tmp = std::env::temp_dir().join("test_no_price.json");
        std::fs::write(&tmp, json).unwrap();
        let opts = ParseOpts::default();
        let results = parse_scrape_files(&[tmp.to_string_lossy().to_string()], &opts);
        assert!(results.is_empty());
    }

    #[test]
    fn test_date_filter() {
        let json = r#"{
            "offers": [
                { "source_id": "besttour", "id": "a1", "url": "https://example.com/a", "price_per_person": 100, "scraped_at": "2026-06-09T00:00:00Z",
                  "best_value": { "date": "2026-06-15", "price_per_person": 100, "price_total": 200 } },
                { "source_id": "besttour", "id": "a2", "url": "https://example.com/b", "price_per_person": 200, "scraped_at": "2026-06-09T00:00:00Z",
                  "best_value": { "date": "2026-07-01", "price_per_person": 200, "price_total": 400 } }
            ]
        }"#;
        let tmp = std::env::temp_dir().join("test_date_filter.json");
        std::fs::write(&tmp, json).unwrap();
        let opts = ParseOpts {
            start: Some("2026-06-10".to_string()),
            end: Some("2026-06-30".to_string()),
            include_undated: true,
            pax: 2,
        };
        let results = parse_scrape_files(&[tmp.to_string_lossy().to_string()], &opts);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].offers.len(), 1);
        assert_eq!(results[0].offers[0].id, "a1");
    }

    #[test]
    fn test_no_json_files() {
        let opts = ParseOpts::default();
        let results = parse_scrape_files(&[], &opts);
        assert!(results.is_empty());
    }
}
