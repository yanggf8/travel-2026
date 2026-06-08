// `travel compare dates` — FIT package vs separate-booking comparison across a
// range of departure dates. Read-only, plain-text (markdown tables, matching the
// TS src/cli/compare-dates.ts byte-for-byte). Reads the offers table + Turso
// holiday calendar; computes leave days via the shared leave module.

use crate::{db, leave};
use chrono::{Duration, NaiveDate};

const USD_TWD: i64 = 32; // EXCHANGE_RATES.USD_TWD
const DEFAULT_LCC_BAGGAGE_FEE: i64 = 1750;
const DEFAULT_PAX: i64 = 2;
const DAY_NAMES_ZH: [&str; 7] = ["日", "一", "二", "三", "四", "五", "六"];

pub struct CompareDatesArgs {
    pub start: String,
    pub end: String,
    pub nights: i64,
    pub hotel_per_night: i64,
    pub market: String,
    pub region: Option<String>,
    pub destination: Option<String>,
    pub pax: i64,
    pub baggage_fee: i64,
}

impl CompareDatesArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut o = CompareDatesArgs {
            start: String::new(),
            end: String::new(),
            nights: 4,
            hotel_per_night: 3200,
            market: "taiwan".to_string(),
            region: None,
            destination: None,
            pax: DEFAULT_PAX,
            baggage_fee: DEFAULT_LCC_BAGGAGE_FEE,
        };
        let mut i = 0;
        while i < args.len() {
            let key = args[i].as_str();
            let val = || {
                args.get(i + 1)
                    .cloned()
                    .ok_or_else(|| format!("{key} requires a value"))
            };
            let int = |v: String, name: &str| v.parse::<i64>().map_err(|_| format!("{name} must be an integer"));
            match key {
                "--start" | "-s" => o.start = val()?,
                "--end" | "-e" => o.end = val()?,
                "--nights" | "-n" => o.nights = int(val()?, "--nights")?,
                "--hotel-per-night" => o.hotel_per_night = int(val()?, "--hotel-per-night")?,
                "--market" | "-m" => o.market = val()?,
                "--region" => o.region = Some(val()?),
                "--destination" => o.destination = Some(val()?),
                "--pax" => o.pax = int(val()?, "--pax")?,
                "--baggage-fee" => o.baggage_fee = int(val()?, "--baggage-fee")?,
                other => return Err(format!("unknown flag for compare dates: {other}")),
            }
            i += 2;
        }
        if o.start.is_empty() || o.end.is_empty() {
            return Err("compare dates requires --start and --end (YYYY-MM-DD)".to_string());
        }
        Ok(o)
    }
}

fn sql_quote(v: &str) -> String {
    v.replace('\'', "''")
}

fn parse_date(s: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| format!("invalid date: {s}"))
}

fn add_days(s: &str, n: i64) -> Result<String, String> {
    Ok((parse_date(s)? + Duration::days(n)).format("%Y-%m-%d").to_string())
}

// JS getDay(): 0=Sun..6=Sat. chrono Weekday: Mon=0..Sun=6 → remap.
fn js_day_of_week(s: &str) -> Result<usize, String> {
    use chrono::Datelike;
    let wd = parse_date(s)?.weekday().num_days_from_sunday() as usize;
    Ok(wd)
}

// Thousands separators, matching JS Number.toLocaleString() for integers.
fn locale(n: i64) -> String {
    let neg = n < 0;
    let digits = n.abs().to_string();
    let bytes = digits.as_bytes();
    let mut out = String::new();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    if neg { format!("-{out}") } else { out }
}

#[derive(Clone)]
struct Offer {
    source_id: String,
    departure_date: String,
    price_per_person: Option<i64>,
    airline: String,
    hotel_name: String,
    name: String,
}

async fn query_offers(
    opts: &CompareDatesArgs,
    kind: &str,
) -> Result<Vec<Offer>, String> {
    let mut conds: Vec<String> = vec![format!("type = '{}'", sql_quote(kind))];
    if let Some(r) = &opts.region {
        conds.push(format!("region = '{}'", sql_quote(r)));
    }
    if let Some(d) = &opts.destination {
        conds.push(format!("destination = '{}'", sql_quote(d)));
    }
    // start..addDays(end, nights) inclusive window (matches TS commonFilters).
    let window_end = add_days(&opts.end, opts.nights)?;
    conds.push(format!("departure_date >= '{}'", sql_quote(&opts.start)));
    conds.push(format!("departure_date <= '{}'", sql_quote(&window_end)));

    let sql = format!(
        "SELECT source_id, departure_date, price_per_person, airline, hotel_name, name \
         FROM offers WHERE {} ORDER BY scraped_at DESC, price_per_person ASC LIMIT 500",
        conds.join(" AND ")
    );

    let conn = db::connect_read().await?;
    let mut rows = conn
        .query(sql.as_str(), ())
        .await
        .map_err(|e| format!("failed to query offers: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| format!("offer row: {e}"))? {
        out.push(Offer {
            source_id: row.get(0).unwrap_or_default(),
            departure_date: row.get(1).unwrap_or_default(),
            price_per_person: row.get(2).ok(),
            airline: row.get(3).unwrap_or_default(),
            hotel_name: row.get(4).unwrap_or_default(),
            name: row.get(5).unwrap_or_default(),
        });
    }
    Ok(out)
}

struct FitPackage {
    price_total_twd: i64,
    price_per_person: i64,
    airline: String,
    hotel: String,
}

struct SeparateBooking {
    out_airline: Option<String>,
    out_twd: Option<i64>,
    ret_airline: Option<String>,
    ret_twd: Option<i64>,
    hotel_total_twd: i64,
    baggage_cost_twd: i64,
    total_twd: i64,
}

struct Comparison {
    depart_date: String,
    return_date: String,
    dow_zh: String,
    leave_days: usize,
    leave_dates: Vec<String>,
    fit: Option<FitPackage>,
    separate: Option<SeparateBooking>,
    fit_vs_separate: Option<i64>,
}

pub async fn run(opts: &CompareDatesArgs) -> Result<(), String> {
    let year: i32 = opts.start.get(0..4).and_then(|y| y.parse().ok()).ok_or("invalid --start year")?;
    let calendar = db::load_holiday_calendar(&opts.market, year).await?;

    let package_offers = query_offers(opts, "package").await?;
    let flight_offers = query_offers(opts, "flight").await?;
    if package_offers.is_empty() && flight_offers.is_empty() {
        return Err("Missing Turso offers for compare-dates. Import scrape output with npm run db:import:turso before running this command.".to_string());
    }

    // cheapest package per departure_date
    let mut fit_by_date: std::collections::HashMap<String, FitPackage> = std::collections::HashMap::new();
    for o in &package_offers {
        let Some(ppp) = o.price_per_person else { continue };
        if o.departure_date.is_empty() {
            continue;
        }
        let entry = fit_by_date.get(&o.departure_date);
        if entry.map(|e| e.price_per_person <= ppp).unwrap_or(false) {
            continue;
        }
        let hotel = if !o.hotel_name.is_empty() { o.hotel_name.clone() } else if !o.name.is_empty() { o.name.clone() } else { "Unknown".to_string() };
        fit_by_date.insert(o.departure_date.clone(), FitPackage {
            price_total_twd: ppp * opts.pax,
            price_per_person: ppp,
            airline: if o.airline.is_empty() { "Unknown".to_string() } else { o.airline.clone() },
            hotel,
        });
    }
    // cheapest flight per departure_date
    let mut flight_by_date: std::collections::HashMap<String, &Offer> = std::collections::HashMap::new();
    for o in &flight_offers {
        let Some(ppp) = o.price_per_person else { continue };
        if o.departure_date.is_empty() {
            continue;
        }
        let better = flight_by_date.get(&o.departure_date).map(|c| c.price_per_person.unwrap_or(i64::MAX) > ppp).unwrap_or(true);
        if better {
            flight_by_date.insert(o.departure_date.clone(), o);
        }
    }

    // departure dates start..=end
    let mut dates = Vec::new();
    let mut cur = opts.start.clone();
    while cur.as_str() <= opts.end.as_str() {
        dates.push(cur.clone());
        cur = add_days(&cur, 1)?;
    }

    let mut comparisons = Vec::new();
    for depart in &dates {
        let return_date = add_days(depart, opts.nights)?;
        let dow = js_day_of_week(depart)?;
        let dow_zh = DAY_NAMES_ZH[dow].to_string();

        let leave = leave::calculate_leave_days(depart, &return_date, &calendar).ok();
        let (leave_days, leave_dates) = match &leave {
            Some(r) => (
                r.leave_days,
                r.breakdown.iter().filter(|d| d.requires_leave).map(|d| {
                    let mm: i64 = d.date[5..7].parse().unwrap_or(0);
                    let dd: i64 = d.date[8..10].parse().unwrap_or(0);
                    format!("{}/{}({})", mm, dd, DAY_NAMES_ZH[d.day_of_week])
                }).collect::<Vec<_>>(),
            ),
            None => (0, Vec::new()),
        };

        let fit = fit_by_date.remove(depart);

        let out_offer = flight_by_date.get(depart).copied();
        let ret_offer = flight_by_date.get(&return_date).copied();
        let separate = if out_offer.is_some() || ret_offer.is_some() {
            let mk = |o: Option<&Offer>| -> (Option<String>, Option<i64>) {
                match o.and_then(|o| o.price_per_person.map(|p| (o, p))) {
                    Some((o, p)) => {
                        let airline = if !o.airline.is_empty() { o.airline.clone() } else { o.source_id.clone() };
                        (Some(airline), Some(p * opts.pax))
                    }
                    None => (None, None),
                }
            };
            let (out_airline, out_twd) = mk(out_offer);
            let (ret_airline, ret_twd) = mk(ret_offer);
            let flight_total = out_twd.unwrap_or(0) + ret_twd.unwrap_or(0);
            let hotel_total = opts.hotel_per_night * opts.nights;
            // baggage charged for each present flight (baggage not included)
            let mut dirs = 0;
            if out_airline.is_some() { dirs += 1; }
            if ret_airline.is_some() { dirs += 1; }
            let baggage = dirs * opts.baggage_fee * opts.pax;
            Some(SeparateBooking {
                out_airline, out_twd, ret_airline, ret_twd,
                hotel_total_twd: hotel_total,
                baggage_cost_twd: baggage,
                total_twd: flight_total + hotel_total + baggage,
            })
        } else {
            None
        };

        let fit_vs_separate = match (&fit, &separate) {
            (Some(f), Some(s)) => Some(f.price_total_twd - s.total_twd),
            _ => None,
        };

        comparisons.push(Comparison {
            depart_date: depart.clone(),
            return_date,
            dow_zh,
            leave_days,
            leave_dates,
            fit,
            separate,
            fit_vs_separate,
        });
    }

    println!("{}", format_comparisons(&comparisons, opts));
    Ok(())
}

fn format_comparisons(comparisons: &[Comparison], opts: &CompareDatesArgs) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("## FIT vs 分開訂 全日期比較 ({}晚, {}人)", opts.nights, opts.pax));
    lines.push(format!(
        "匯率: 1 USD ≈ {} TWD | 飯店估: TWD {}/晚 | 行李: TWD {}/人/方向",
        USD_TWD, opts.hotel_per_night, opts.baggage_fee
    ));
    lines.push(String::new());

    lines.push("| 出發 | 回程 | 請假 | FIT(2人) | 分開訂(2人) | 差額 | 贏家 |".to_string());
    lines.push("|------|------|:----:|--------:|-----------:|-----:|------|".to_string());
    for c in comparisons {
        let dep = format!("{}({})", &c.depart_date[5..], c.dow_zh);
        let ret = &c.return_date[5..];
        let fit_price = c.fit.as_ref().map(|f| locale(f.price_total_twd)).unwrap_or_else(|| "(無資料)".to_string());
        let sep_price = c.separate.as_ref().map(|s| format!("~{}", locale(s.total_twd))).unwrap_or_else(|| "(無資料)".to_string());
        let (diff, winner) = match c.fit_vs_separate {
            Some(d) if d > 0 => (format!("+{}", locale(d)), "分開訂"),
            Some(d) if d < 0 => (locale(d), "FIT"),
            Some(_) => ("0".to_string(), "平手"),
            None => ("-".to_string(), "-"),
        };
        lines.push(format!("| {} | {} | {}天 | {} | {} | {} | {} |", dep, ret, c.leave_days, fit_price, sep_price, diff, winner));
    }

    lines.push(String::new());
    lines.push("### 分開訂明細".to_string());
    lines.push(String::new());
    lines.push("| 出發 | 去程 | TWD | 回程 | TWD | 飯店 | 行李 | 合計 |".to_string());
    lines.push("|------|------|----:|------|----:|-----:|-----:|-----:|".to_string());
    for c in comparisons {
        let dep = format!("{}({})", &c.depart_date[5..], c.dow_zh);
        match &c.separate {
            None => lines.push(format!("| {} | - | - | - | - | - | - | (無資料) |", dep)),
            Some(s) => {
                let out_airline = s.out_airline.as_ref().map(|a| truncate(a, 12)).unwrap_or_else(|| "-".to_string());
                let out_twd = s.out_twd.map(locale).unwrap_or_else(|| "-".to_string());
                let ret_airline = s.ret_airline.as_ref().map(|a| truncate(a, 12)).unwrap_or_else(|| "-".to_string());
                let ret_twd = s.ret_twd.map(locale).unwrap_or_else(|| "-".to_string());
                let bag = if s.baggage_cost_twd > 0 { locale(s.baggage_cost_twd) } else { "0".to_string() };
                lines.push(format!("| {} | {} | {} | {} | {} | {} | {} | {} |", dep, out_airline, out_twd, ret_airline, ret_twd, locale(s.hotel_total_twd), bag, locale(s.total_twd)));
            }
        }
    }

    lines.push(String::new());
    lines.push("### FIT 明細".to_string());
    lines.push(String::new());
    lines.push("| 出發 | 價格(2人) | 每人 | 航空 | 飯店 | 行李 |".to_string());
    lines.push("|------|--------:|-----:|------|------|------|".to_string());
    for c in comparisons {
        let dep = format!("{}({})", &c.depart_date[5..], c.dow_zh);
        match &c.fit {
            None => lines.push(format!("| {} | (無資料) | - | - | - | - |", dep)),
            Some(f) => lines.push(format!("| {} | {} | {} | {} | {} | 含20kg |", dep, locale(f.price_total_twd), locale(f.price_per_person), f.airline, f.hotel)),
        }
    }

    lines.push(String::new());
    lines.push("### 請假明細".to_string());
    lines.push(String::new());
    lines.push("| 出發 | 請假天數 | 請假日期 |".to_string());
    lines.push("|------|:-------:|---------|".to_string());
    for c in comparisons {
        let dep = format!("{}({})", &c.depart_date[5..], c.dow_zh);
        let dates = if c.leave_dates.is_empty() { "不用請假".to_string() } else { c.leave_dates.join(", ") };
        lines.push(format!("| {} | {}天 | {} |", dep, c.leave_days, dates));
    }

    let with_both = comparisons.iter().filter(|c| c.fit.is_some() && c.separate.is_some()).count();
    if with_both > 0 {
        let cheapest_fit = comparisons.iter().filter(|c| c.fit.is_some()).min_by_key(|c| c.fit.as_ref().unwrap().price_total_twd);
        let cheapest_sep = comparisons.iter().filter(|c| c.separate.is_some()).min_by_key(|c| c.separate.as_ref().unwrap().total_twd);
        let least_leave = comparisons.iter().min_by_key(|c| c.leave_days);
        lines.push(String::new());
        lines.push("### 建議".to_string());
        if let Some(c) = cheapest_fit {
            lines.push(format!("- **最便宜FIT**: {}({}) TWD {}", &c.depart_date[5..], c.dow_zh, locale(c.fit.as_ref().unwrap().price_total_twd)));
        }
        if let Some(c) = cheapest_sep {
            lines.push(format!("- **最便宜分開訂**: {}({}) ~TWD {}", &c.depart_date[5..], c.dow_zh, locale(c.separate.as_ref().unwrap().total_twd)));
        }
        if let Some(c) = least_leave {
            lines.push(format!("- **最少請假**: {}({}) {}天", &c.depart_date[5..], c.dow_zh, c.leave_days));
        }
    }

    lines.join("\n")
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
