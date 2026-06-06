#![allow(dead_code)]

use crate::leave::{HolidayCalendar, LeaveDayResult, calculate_leave_days};

#[derive(Debug, Clone)]
pub struct CompareArgs {
    pub trips: Vec<TripOption>,
    pub market: String,
    pub detailed: bool,
    pub default_baggage_fee: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TripKind {
    Package,
    Separate,
}

#[derive(Debug, Clone)]
pub struct FlightInfo {
    pub airline: String,
    pub is_lcc: bool,
}

#[derive(Debug, Clone)]
pub struct HotelInfo {
    pub name: String,
    pub nights: i64,
}

#[derive(Debug, Clone)]
pub struct TripOption {
    pub id: String,
    pub kind: TripKind,
    pub start_date: String,
    pub end_date: String,
    pub pax: i64,
    pub package_price: i64,
    pub outbound_flight: Option<FlightInfo>,
    pub return_flight: Option<FlightInfo>,
    pub flight_price_total: i64,
    pub hotel: Option<HotelInfo>,
    pub hotel_price_total: i64,
    pub baggage_fee: Option<i64>,
    pub currency: String,
    pub notes: Option<String>,
}

#[derive(Debug, Clone)]
struct TripAnalysis {
    id: String,
    kind: TripKind,
    start_date: String,
    end_date: String,
    total_days: usize,
    leave_days: usize,
    pax: i64,
    base_price: i64,
    baggage_fee: i64,
    total_price: i64,
    price_per_person: f64,
    price_per_leave_day: f64,
    flight_info: Option<String>,
    hotel_info: Option<String>,
    notes: Option<String>,
    _leave_breakdown: LeaveDayResult,
}

const DEFAULT_BAGGAGE_FEE: i64 = 1750;

impl CompareArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut trips = Vec::new();
        let mut market = "taiwan".to_string();
        let mut detailed = false;
        let mut default_baggage_fee = DEFAULT_BAGGAGE_FEE;

        let mut i = 0usize;
        while i < args.len() {
            match args[i].as_str() {
                "--trip" | "-t" => {
                    i += 1;
                    let spec = args
                        .get(i)
                        .ok_or_else(|| "missing value after --trip".to_string())?;
                    trips.push(parse_trip_spec(spec)?);
                }
                "--market" | "-m" => {
                    i += 1;
                    market = args
                        .get(i)
                        .ok_or_else(|| "missing value after --market".to_string())?
                        .to_string();
                }
                "--detailed" | "-d" => detailed = true,
                "--baggage-fee" => {
                    i += 1;
                    default_baggage_fee = parse_i64(
                        args.get(i)
                            .ok_or_else(|| "missing value after --baggage-fee".to_string())?,
                        "baggage-fee",
                    )?;
                }
                other => return Err(format!("unknown compare trips argument: {other}")),
            }
            i += 1;
        }

        if trips.is_empty() {
            return Err("missing --trip; pass one or more plain-text trip specs".to_string());
        }

        Ok(Self {
            trips,
            market,
            detailed,
            default_baggage_fee,
        })
    }

    pub fn year(&self) -> Result<i32, String> {
        let first = self
            .trips
            .first()
            .ok_or_else(|| "No trips provided".to_string())?;
        first
            .start_date
            .get(0..4)
            .ok_or_else(|| format!("invalid start date: {}", first.start_date))?
            .parse::<i32>()
            .map_err(|err| format!("invalid start date year: {err}"))
    }
}

pub fn compare_trips(args: &CompareArgs, calendar: &HolidayCalendar) -> Result<String, String> {
    let mut analyses = Vec::new();
    for trip in &args.trips {
        analyses.push(analyze_trip(trip, calendar, args.default_baggage_fee)?);
    }

    let best_value = analyses
        .iter()
        .min_by(|a, b| a.price_per_leave_day.total_cmp(&b.price_per_leave_day))
        .ok_or_else(|| "No trips provided".to_string())?;
    let least_leave = analyses
        .iter()
        .min_by_key(|a| a.leave_days)
        .ok_or_else(|| "No trips provided".to_string())?;
    let cheapest = analyses
        .iter()
        .min_by_key(|a| a.total_price)
        .ok_or_else(|| "No trips provided".to_string())?;

    let mut lines = vec![
        "## Trip Comparison Summary\n".to_string(),
        "| Option | Type | Dates | Leave | Total | $/Leave |".to_string(),
        "|--------|------|-------|:-----:|------:|--------:|".to_string(),
    ];

    for a in &analyses {
        let dates = format!("{} → {}", &a.start_date[5..], &a.end_date[5..]);
        let type_label = if a.kind == TripKind::Package {
            "套餐"
        } else {
            "分開訂"
        };
        lines.push(format!(
            "| {} | {} | {} | {}天 | {} | {} |",
            a.id,
            type_label,
            dates,
            a.leave_days,
            format_int(a.total_price),
            format_int(a.price_per_leave_day.round() as i64)
        ));
    }

    lines.push(String::new());
    lines.push("### Recommendations".to_string());
    lines.push(format!("- **Best Value ($/Leave)**: {}", best_value.id));
    lines.push(format!("- **Least Leave Days**: {}", least_leave.id));
    lines.push(format!("- **Cheapest Total**: {}", cheapest.id));

    if args.detailed {
        lines.push("\n## Detailed Breakdown\n".to_string());
        for analysis in &analyses {
            lines.push(format_detailed_analysis(analysis));
            lines.push(String::new());
        }
    }

    Ok(lines.join("\n"))
}

pub fn print_usage() {
    println!(
        "Trip Comparison CLI\n\nUsage:\n  travel compare trips --trip '<key=value;...>' --trip '<key=value;...>' [--market taiwan] [--detailed]\n\nTrip spec keys:\n  id, type=package|separate, start, end, pax, currency, notes\n  package price: package=<total>\n  separate price: flight_total=<total>;hotel_total=<total>;hotel=<name>;hotel_nights=<n>\n  flights: outbound=<airline>;return=<airline>;lcc_out=true|false;lcc_return=true|false\n\nExample:\n  travel compare trips --trip 'id=Pkg;type=package;start=2026-06-20;end=2026-06-24;pax=2;package=36587;currency=TWD'"
    );
}

fn analyze_trip(
    option: &TripOption,
    calendar: &HolidayCalendar,
    default_baggage_fee: i64,
) -> Result<TripAnalysis, String> {
    let leave = calculate_leave_days(&option.start_date, &option.end_date, calendar)
        .map_err(|err| format!("Failed to calculate leave for {}: {err}", option.id))?;

    let mut baggage_fee = 0i64;
    let mut flight_info = None;
    let mut hotel_info = None;

    let base_price = if option.kind == TripKind::Package {
        option.package_price
    } else {
        if option
            .outbound_flight
            .as_ref()
            .map(|flight| flight.is_lcc)
            .unwrap_or(false)
            || option
                .return_flight
                .as_ref()
                .map(|flight| flight.is_lcc)
                .unwrap_or(false)
        {
            let directions = option
                .outbound_flight
                .as_ref()
                .map(|flight| i64::from(flight.is_lcc))
                .unwrap_or(0)
                + option
                    .return_flight
                    .as_ref()
                    .map(|flight| i64::from(flight.is_lcc))
                    .unwrap_or(0);
            baggage_fee =
                option.baggage_fee.unwrap_or(default_baggage_fee) * directions * option.pax;
        }

        if let (Some(out), Some(ret)) = (&option.outbound_flight, &option.return_flight) {
            flight_info = Some(format!("{} → {}", out.airline, ret.airline));
        }
        if let Some(hotel) = &option.hotel {
            hotel_info = Some(format!("{} ({}晚)", hotel.name, hotel.nights));
        }
        option.flight_price_total + option.hotel_price_total
    };

    let total_price = base_price + baggage_fee;
    let price_per_person = total_price as f64 / option.pax as f64;
    let price_per_leave_day = if leave.leave_days > 0 {
        total_price as f64 / leave.leave_days as f64
    } else {
        total_price as f64
    };

    Ok(TripAnalysis {
        id: option.id.clone(),
        kind: option.kind.clone(),
        start_date: option.start_date.clone(),
        end_date: option.end_date.clone(),
        total_days: leave.total_days,
        leave_days: leave.leave_days,
        pax: option.pax,
        base_price,
        baggage_fee,
        total_price,
        price_per_person,
        price_per_leave_day,
        flight_info,
        hotel_info,
        notes: option.notes.clone(),
        _leave_breakdown: leave,
    })
}

fn format_detailed_analysis(analysis: &TripAnalysis) -> String {
    let mut lines = Vec::new();
    lines.push(format!("### {}", analysis.id));
    lines.push(format!(
        "- **Type**: {}",
        if analysis.kind == TripKind::Package {
            "套餐"
        } else {
            "分開訂"
        }
    ));
    lines.push(format!(
        "- **Dates**: {} → {} ({} days)",
        analysis.start_date, analysis.end_date, analysis.total_days
    ));
    lines.push(format!("- **Leave Days**: {}", analysis.leave_days));
    if let Some(flight) = &analysis.flight_info {
        lines.push(format!("- **Flight**: {flight}"));
    }
    if let Some(hotel) = &analysis.hotel_info {
        lines.push(format!("- **Hotel**: {hotel}"));
    }
    lines.push(format!(
        "- **Base Price**: {}",
        format_int(analysis.base_price)
    ));
    if analysis.baggage_fee > 0 {
        lines.push(format!(
            "- **Baggage Fee**: +{} (LCC)",
            format_int(analysis.baggage_fee)
        ));
    }
    lines.push(format!("- **Total**: {}", format_int(analysis.total_price)));
    lines.push(format!(
        "- **Per Person**: {}",
        format_number(analysis.price_per_person)
    ));
    lines.push(format!(
        "- **Per Leave Day**: {}",
        format_int(analysis.price_per_leave_day.round() as i64)
    ));
    if let Some(notes) = &analysis.notes {
        lines.push(format!("- **Notes**: {notes}"));
    }
    lines.join("\n")
}

fn parse_trip_spec(spec: &str) -> Result<TripOption, String> {
    let mut fields = Vec::new();
    for part in spec.split(';') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (key, value) = trimmed
            .split_once('=')
            .ok_or_else(|| format!("trip spec field must be key=value: {trimmed}"))?;
        fields.push((key.trim().to_ascii_lowercase(), value.trim().to_string()));
    }

    let get = |name: &str| -> Option<String> {
        fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    let get_any = |names: &[&str]| -> Option<String> {
        names.iter().find_map(|name| {
            fields
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        })
    };

    let id = get("id").ok_or_else(|| "trip spec missing id".to_string())?;
    let kind = match get("type")
        .unwrap_or_else(|| "package".to_string())
        .to_ascii_lowercase()
        .as_str()
    {
        "package" | "pkg" => TripKind::Package,
        "separate" | "sep" => TripKind::Separate,
        other => return Err(format!("unsupported trip type for {id}: {other}")),
    };
    let start_date =
        get_any(&["start", "startdate"]).ok_or_else(|| format!("{id} missing start"))?;
    let end_date = get_any(&["end", "enddate"]).ok_or_else(|| format!("{id} missing end"))?;
    let pax = parse_i64(&get("pax").unwrap_or_else(|| "2".to_string()), "pax")?;
    let package_price = get_any(&["package", "packageprice", "package_price"])
        .map(|v| parse_i64(&v, "package"))
        .transpose()?
        .unwrap_or(0);
    let flight_price_total = get_any(&["flight_total", "flightpricetotal"])
        .map(|v| parse_i64(&v, "flight_total"))
        .transpose()?
        .unwrap_or(0);
    let hotel_price_total = get_any(&["hotel_total", "hotelpricetotal"])
        .map(|v| parse_i64(&v, "hotel_total"))
        .transpose()?
        .unwrap_or(0);
    let hotel_nights = get_any(&["hotel_nights", "nights"])
        .map(|v| parse_i64(&v, "hotel_nights"))
        .transpose()?
        .unwrap_or(0);
    let hotel = get("hotel").map(|name| HotelInfo {
        name,
        nights: hotel_nights,
    });
    let outbound_flight = get_any(&["outbound", "outbound_airline"]).map(|airline| FlightInfo {
        airline,
        is_lcc: parse_bool(
            get_any(&["lcc_out", "outbound_lcc"])
                .as_deref()
                .unwrap_or("false"),
        ),
    });
    let return_flight = get_any(&["return", "return_airline"]).map(|airline| FlightInfo {
        airline,
        is_lcc: parse_bool(
            get_any(&["lcc_return", "return_lcc"])
                .as_deref()
                .unwrap_or("false"),
        ),
    });
    let baggage_fee = get_any(&["baggage", "baggage_fee"])
        .map(|v| parse_i64(&v, "baggage"))
        .transpose()?;

    Ok(TripOption {
        id,
        kind,
        start_date,
        end_date,
        pax,
        package_price,
        outbound_flight,
        return_flight,
        flight_price_total,
        hotel,
        hotel_price_total,
        baggage_fee,
        currency: get("currency").unwrap_or_else(|| "TWD".to_string()),
        notes: get("notes"),
    })
}

fn parse_bool(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "y"
    )
}

fn parse_i64(value: &str, label: &str) -> Result<i64, String> {
    value
        .replace(',', "")
        .parse::<i64>()
        .map_err(|err| format!("invalid {label}: {value}: {err}"))
}

fn format_number(value: f64) -> String {
    if value.fract().abs() < f64::EPSILON {
        format_int(value as i64)
    } else {
        let rounded = (value * 100.0).round() / 100.0;
        let raw = format!("{rounded:.2}");
        let trimmed = raw.trim_end_matches('0').trim_end_matches('.');
        let (whole, frac) = trimmed.split_once('.').unwrap_or((trimmed, ""));
        if frac.is_empty() {
            format_int(whole.parse::<i64>().unwrap_or(0))
        } else {
            format!("{}.{}", format_int(whole.parse::<i64>().unwrap_or(0)), frac)
        }
    }
}

fn format_int(value: i64) -> String {
    let negative = value < 0;
    let digits = value.abs().to_string();
    let mut out = String::new();
    for (idx, ch) in digits.chars().rev().enumerate() {
        if idx > 0 && idx % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    let mut formatted: String = out.chars().rev().collect();
    if negative {
        formatted.insert(0, '-');
    }
    formatted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::leave::HolidayCalendar;
    use std::collections::HashMap;

    fn calendar() -> HolidayCalendar {
        HolidayCalendar {
            country: "taiwan".to_string(),
            year: 2026,
            description: "test".to_string(),
            holidays: HashMap::new(),
            makeup_workdays: HashMap::new(),
        }
    }

    #[test]
    fn parses_plain_text_trip_specs() {
        let trip = parse_trip_spec("id=Sep;type=separate;start=2026-06-20;end=2026-06-24;pax=2;outbound=Peach;return=Tigerair Taiwan;lcc_out=true;lcc_return=true;flight_total=26000;hotel=Smile Hotel;hotel_nights=4;hotel_total=12000;currency=TWD").unwrap();
        assert_eq!(trip.id, "Sep");
        assert_eq!(trip.kind, TripKind::Separate);
        assert_eq!(trip.hotel.unwrap().nights, 4);
        assert!(trip.outbound_flight.unwrap().is_lcc);
    }

    #[test]
    fn compares_trips_with_ts_matching_summary_shape() {
        let args = CompareArgs::parse(&[
            "--trip".to_string(),
            "id=Pkg;type=package;start=2026-06-20;end=2026-06-24;pax=2;package=36587;currency=TWD".to_string(),
            "--trip".to_string(),
            "id=Sep;type=separate;start=2026-06-20;end=2026-06-24;pax=2;outbound=Peach;return=Tigerair Taiwan;lcc_out=true;lcc_return=true;flight_total=26000;hotel=Smile Hotel;hotel_nights=4;hotel_total=12000;currency=TWD".to_string(),
        ]).unwrap();
        let out = compare_trips(&args, &calendar()).unwrap();
        assert!(out.contains("| Pkg | 套餐 | 06-20 → 06-24 | 3天 | 36,587 | 12,196 |"));
        assert!(out.contains("- **Cheapest Total**: Pkg"));
    }
}
