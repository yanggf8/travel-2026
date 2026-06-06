#![allow(dead_code)]

use regex::Regex;

#[derive(Debug, Clone)]
pub struct NormalizeArgs {
    pub text: Option<String>,
    pub stdin: bool,
    pub url: Option<String>,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct NormalizedFlight {
    pub airline: String,
    pub dep_time: String,
    pub arr_time: String,
    pub dep_airport: String,
    pub arr_airport: String,
    pub duration: String,
    pub price_per_person: i64,
    pub price_total: i64,
    pub price_total_twd: i64,
    pub baggage_included: bool,
    pub is_lcc: bool,
}

#[derive(Debug, Clone)]
pub struct FlightSearchResult {
    pub label: String,
    pub direction: String,
    pub date: String,
    pub pax: i64,
    pub flights: Vec<NormalizedFlight>,
    pub cheapest_lcc: Option<NormalizedFlight>,
    pub cheapest_full: Option<NormalizedFlight>,
    pub cheapest_any: Option<NormalizedFlight>,
}

const LCC_AIRLINES: [&str; 8] = [
    "Peach",
    "Tigerair Taiwan",
    "Jetstar Japan",
    "AirAsia X Berhad",
    "Thai Vietjet Air",
    "Thai Lion Air",
    "HK Express",
    "Scoot",
];

impl NormalizeArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut text = None;
        let mut stdin = false;
        let mut url = None;
        let mut label = "stdin".to_string();
        let mut i = 0usize;
        while i < args.len() {
            match args[i].as_str() {
                "--text" => {
                    i += 1;
                    text = Some(
                        args.get(i)
                            .ok_or_else(|| "missing value after --text".to_string())?
                            .to_string(),
                    );
                }
                "--stdin" => stdin = true,
                "--url" => {
                    i += 1;
                    url = Some(
                        args.get(i)
                            .ok_or_else(|| "missing value after --url".to_string())?
                            .to_string(),
                    );
                }
                "--label" => {
                    i += 1;
                    label = args
                        .get(i)
                        .ok_or_else(|| "missing value after --label".to_string())?
                        .to_string();
                }
                other => return Err(format!("unknown normalize flights argument: {other}")),
            }
            i += 1;
        }
        if stdin && text.is_some() {
            return Err("pass either --stdin or --text, not both".to_string());
        }
        Ok(Self {
            text,
            stdin,
            url,
            label,
        })
    }
}

pub fn normalize_text(
    label: &str,
    url: &str,
    raw_text: &str,
) -> Result<FlightSearchResult, String> {
    if raw_text.trim().is_empty() {
        return Err(format!("No raw_text found in {label}"));
    }
    let pax = detect_pax(url);
    let (direction, date) = detect_direction_and_date(url);
    let flights = parse_flights(raw_text)?;
    let cheapest_lcc = flights.iter().find(|flight| flight.is_lcc).cloned();
    let cheapest_full = flights.iter().find(|flight| !flight.is_lcc).cloned();
    let cheapest_any = flights.first().cloned();
    Ok(FlightSearchResult {
        label: label.to_string(),
        direction,
        date,
        pax,
        flights,
        cheapest_lcc,
        cheapest_full,
        cheapest_any,
    })
}

pub fn format_search_result(result: &FlightSearchResult) -> String {
    let route = if result.direction == "outbound" {
        "TPE→KIX"
    } else {
        "KIX→TPE"
    };
    let cheapest = result
        .cheapest_any
        .as_ref()
        .map(format_flight)
        .unwrap_or_else(|| "none".to_string());
    format!(
        "=== {} Flights ({}) ===\n  {}: {} flights, cheapest: {}",
        title_case(&result.direction),
        route,
        result.date,
        result.flights.len(),
        cheapest
    )
}

pub fn print_usage() {
    println!(
        "Flight Data Normalizer\n\nUsage:\n  travel normalize flights --text '<rendered flight text>' --url '<source url>' [--label name]\n  travel normalize flights --stdin --url '<source url>' [--label name]\n\nNotes:\n  This Rust command normalizes plain rendered text. It does not read local JSON files or legacy raw_data JSON."
    );
}

fn parse_flights(raw_text: &str) -> Result<Vec<NormalizedFlight>, String> {
    let flight_re = Regex::new(
        r"(?s)(?:^|\n)([A-Za-z][\w\s]+?)\n(\d{2}:\d{2})\n([A-Z]{3} T\d)\n(\d+h \d+m)\nNonstop\n(\d{2}:\d{2})\n([A-Z]{3} T\d)",
    )
    .map_err(|err| format!("invalid flight regex: {err}"))?;
    let price_re = Regex::new(r"US\$(\d[\d,]*)\nTotal US\$(\d[\d,]*)")
        .map_err(|err| format!("invalid price regex: {err}"))?;
    let mut flights = Vec::new();

    for section in raw_text.split("\nSelect\n") {
        if !section.contains("Nonstop") {
            continue;
        }
        let baggage_included = section
            .lines()
            .map(str::trim)
            .find_map(|line| {
                if line == "Included" {
                    Some(true)
                } else if line == "Carry-on baggage included" {
                    Some(false)
                } else {
                    None
                }
            })
            .unwrap_or(false);
        let Some(flight_match) = flight_re.captures(section) else {
            continue;
        };
        let Some(price_match) = price_re.captures(section) else {
            continue;
        };
        let airline = flight_match[1]
            .trim()
            .lines()
            .last()
            .unwrap_or("")
            .trim()
            .to_string();
        let price_per_person = parse_amount(&price_match[1])?;
        let price_total = parse_amount(&price_match[2])?;
        flights.push(NormalizedFlight {
            is_lcc: is_lcc_airline(&airline),
            airline,
            dep_time: flight_match[2].to_string(),
            dep_airport: flight_match[3].to_string(),
            duration: flight_match[4].to_string(),
            arr_time: flight_match[5].to_string(),
            arr_airport: flight_match[6].to_string(),
            price_per_person,
            price_total,
            price_total_twd: price_total * 32,
            baggage_included,
        });
    }

    flights.sort_by_key(|flight| flight.price_total_twd);
    Ok(flights)
}

fn format_flight(flight: &NormalizedFlight) -> String {
    let bag = if flight.baggage_included {
        "bag"
    } else {
        "carry"
    };
    let kind = if flight.is_lcc { "LCC" } else { "FSC" };
    format!(
        "{} {}→{} US${}(2p) TWD{} [{}/{}]",
        flight.airline,
        flight.dep_time,
        flight.arr_time,
        flight.price_total,
        flight.price_total_twd,
        kind,
        bag
    )
}

fn detect_pax(url: &str) -> i64 {
    Regex::new(r"quantity=(\d+)")
        .ok()
        .and_then(|rx| rx.captures(url))
        .and_then(|caps| caps.get(1).and_then(|m| m.as_str().parse::<i64>().ok()))
        .unwrap_or(2)
}

fn detect_direction_and_date(url: &str) -> (String, String) {
    let direction = if url.contains("tpe-kix") || url.contains("dcity=tpe") {
        "outbound"
    } else {
        "return"
    };
    let date = Regex::new(r"ddate=(\d{4}-\d{2}-\d{2})")
        .ok()
        .and_then(|rx| rx.captures(url))
        .and_then(|caps| caps.get(1).map(|m| m.as_str().to_string()))
        .unwrap_or_else(|| "unknown".to_string());
    (direction.to_string(), date)
}

fn is_lcc_airline(airline: &str) -> bool {
    LCC_AIRLINES.contains(&airline)
}

fn parse_amount(value: &str) -> Result<i64, String> {
    value
        .replace(',', "")
        .parse::<i64>()
        .map_err(|err| format!("invalid amount {value}: {err}"))
}

fn title_case(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAW: &str = "Carry-on baggage included\nPeach\n08:15\nTPE T1\n2h 35m\nNonstop\n11:50\nKIX T2\nUS$180\nTotal US$360\nSelect\nIncluded\nEVA Air\n09:00\nTPE T2\n2h 40m\nNonstop\n12:40\nKIX T1\nUS$250\nTotal US$500";

    #[test]
    fn parses_nonstop_flights_from_plain_text() {
        let result = normalize_text(
            "sample",
            "https://trip.com/flights/tpe-kix?ddate=2026-06-20&quantity=2",
            RAW,
        )
        .unwrap();
        assert_eq!(result.direction, "outbound");
        assert_eq!(result.date, "2026-06-20");
        assert_eq!(result.flights.len(), 2);
        assert_eq!(result.cheapest_any.unwrap().airline, "Peach");
    }

    #[test]
    fn formats_summary_as_plain_text() {
        let result = normalize_text(
            "sample",
            "https://trip.com/flights/tpe-kix?ddate=2026-06-20&quantity=2",
            RAW,
        )
        .unwrap();
        let out = format_search_result(&result);
        assert!(out.contains("=== Outbound Flights (TPE→KIX) ==="));
        assert!(out.contains(
            "2026-06-20: 2 flights, cheapest: Peach 08:15→11:50 US$360(2p) TWD11520 [LCC/carry]"
        ));
    }
}
