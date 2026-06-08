// `travel compare true-cost` — compare package offers by TOTAL cost (package +
// baggage surcharge + transport). Read-only, plain-text box tables, byte-parity
// with src/cli/compare-true-cost.ts. Reads offers + transport_routes/hubs +
// airlines + hotel_area_keywords (the de-JSON'd child table) from Turso.

use crate::db;
use std::collections::BTreeMap;

const DEFAULT_PAX: i64 = 2;
const DEFAULT_JPY_TWD: f64 = 0.22;

pub struct TrueCostArgs {
    pub region: String,
    pub date: Option<String>,
    pub pax: i64,
    pub itinerary: Option<String>,
    pub jpy_rate: f64,
}

impl TrueCostArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut region: Option<String> = None;
        let mut date = None;
        let mut pax = DEFAULT_PAX;
        let mut itinerary = None;
        let mut jpy_rate = DEFAULT_JPY_TWD;
        let mut i = 0;
        while i < args.len() {
            let key = args[i].as_str();
            let val = || args.get(i + 1).cloned().ok_or_else(|| format!("{key} requires a value"));
            match key {
                "--region" => region = Some(val()?),
                "--date" => date = Some(val()?),
                "--pax" => pax = val()?.parse().map_err(|_| "--pax must be an integer".to_string())?,
                "--itinerary" => itinerary = Some(val()?),
                "--jpy-rate" => jpy_rate = val()?.parse().map_err(|_| "--jpy-rate must be a number".to_string())?,
                other => return Err(format!("unknown flag for compare true-cost: {other}")),
            }
            i += 2;
        }
        let region = region.ok_or_else(|| {
            "Usage: compare-true-cost --region <name> [--date YYYY-MM-DD] [--pax N] [--itinerary \"kyoto:1,osaka:2\"] [--jpy-rate N]".to_string()
        })?;
        Ok(TrueCostArgs { region, date, pax, itinerary, jpy_rate })
    }
}

fn sql_quote(v: &str) -> String {
    v.replace('\'', "''")
}

// JS Math.round: half rounds toward +Infinity (not away-from-zero).
fn js_round(x: f64) -> i64 {
    (x + 0.5).floor() as i64
}

// locale string (thousands separators) for integers — matches Number.toLocaleString.
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

// pad helpers operating on char count (JS string length), right/left.
fn pad_end(s: &str, n: usize) -> String {
    let len = s.chars().count();
    if len >= n { s.to_string() } else { format!("{}{}", s, " ".repeat(n - len)) }
}
fn pad_start(s: &str, n: usize) -> String {
    let len = s.chars().count();
    if len >= n { s.to_string() } else { format!("{}{}", " ".repeat(n - len), s) }
}
fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

struct TransportRoute {
    time_min: i64,
    cost_jpy: i64,
}

struct RegionData {
    routes: BTreeMap<String, TransportRoute>,
    hubs: Vec<(String, String, String)>, // (hub_id, hub_type, area) — insertion order
}

pub async fn run(opts: &TrueCostArgs) -> Result<(), String> {
    let conn = db::connect_read().await?;

    // NOTE: scanOffers hardcodes package_type='fit'/baggage_included=null, so the
    // TS calcBaggageCost always returns 0 here and never consults the airlines
    // table. We therefore skip loading airlines entirely (parity-equivalent).

    // --- transport routes + hubs ---
    let mut regions: BTreeMap<String, RegionData> = BTreeMap::new();
    {
        let mut rows = conn.query("SELECT region, route_key, time_min, cost_jpy FROM transport_routes ORDER BY region, route_key", ()).await.map_err(|e| format!("routes query: {e}"))?;
        while let Some(r) = rows.next().await.map_err(|e| format!("route row: {e}"))? {
            let region: String = r.get(0).unwrap_or_default();
            let key: String = r.get(1).unwrap_or_default();
            let entry = regions.entry(region).or_insert_with(|| RegionData { routes: BTreeMap::new(), hubs: Vec::new() });
            entry.routes.insert(key, TransportRoute { time_min: r.get(2).unwrap_or(0), cost_jpy: r.get(3).unwrap_or(0) });
        }
        let mut rows = conn.query("SELECT region, hub_id, hub_type, area FROM transport_hubs ORDER BY region, hub_id", ()).await.map_err(|e| format!("hubs query: {e}"))?;
        while let Some(r) = rows.next().await.map_err(|e| format!("hub row: {e}"))? {
            let region: String = r.get(0).unwrap_or_default();
            let entry = regions.entry(region).or_insert_with(|| RegionData { routes: BTreeMap::new(), hubs: Vec::new() });
            entry.hubs.push((r.get(1).unwrap_or_default(), r.get(2).unwrap_or_default(), r.get(3).unwrap_or_default()));
        }
    }

    // --- hotel area keywords (de-JSON'd child table), grouped region → areaType → [kw] ---
    // BTreeMap keeps areaType order stable; within an area, sort_order preserved by ORDER BY.
    let mut hotel_areas: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    {
        let mut rows = conn.query("SELECT region, area_type, keyword FROM hotel_area_keywords ORDER BY region, area_type, sort_order", ()).await.map_err(|e| format!("hotel_area_keywords query: {e}"))?;
        while let Some(r) = rows.next().await.map_err(|e| format!("hotel area row: {e}"))? {
            let region: String = r.get(0).unwrap_or_default();
            let area_type: String = r.get(1).unwrap_or_default();
            let kw: String = r.get(2).unwrap_or_default();
            hotel_areas.entry(region).or_default().entry(area_type).or_default().push(kw);
        }
    }

    // --- offers (package, region, optional exact date) ---
    let mut conds = vec![format!("region = '{}'", sql_quote(&opts.region)), "type = 'package'".to_string()];
    if let Some(d) = &opts.date {
        conds.push(format!("departure_date >= '{}'", sql_quote(d)));
        conds.push(format!("departure_date <= '{}'", sql_quote(d)));
    }
    let sql = format!(
        "SELECT id, source_id, price_per_person, currency, airline, hotel_name, name FROM offers \
         WHERE {} ORDER BY scraped_at DESC, price_per_person ASC LIMIT 500",
        conds.join(" AND ")
    );
    let mut offers: Vec<RawOffer> = Vec::new();
    {
        let mut rows = conn.query(sql.as_str(), ()).await.map_err(|e| format!("offers query: {e}"))?;
        while let Some(r) = rows.next().await.map_err(|e| format!("offer row: {e}"))? {
            let ppp: Option<i64> = r.get(2).ok();
            let Some(price) = ppp else { continue };
            let hotel_name: String = r.get(5).unwrap_or_default();
            let name: String = r.get(6).unwrap_or_default();
            offers.push(RawOffer {
                file: r.get(0).unwrap_or_default(),
                source_id: r.get(1).unwrap_or_default(),
                price_per_person: price,
                currency: { let c: String = r.get(3).unwrap_or_default(); if c.is_empty() { "TWD".to_string() } else { c } },
                airline: r.get(4).unwrap_or_default(),
                hotel: if !hotel_name.is_empty() { hotel_name } else { name },
            });
        }
    }
    // sort by price asc (stable) — matches TS scanOffers .sort
    offers.sort_by_key(|o| o.price_per_person);

    if offers.is_empty() {
        println!("No offers found for region \"{}\".", opts.region);
        std::process::exit(1);
    }

    let itinerary = parse_itinerary(opts.itinerary.as_deref());

    let mut results: Vec<TrueCostOffer> = Vec::new();
    for o in &offers {
        let area_type = if !o.hotel.is_empty() { detect_hotel_area(&o.hotel, &opts.region, &hotel_areas) } else { "unknown".to_string() };
        let hotel_hub = area_type_to_hub(&area_type, &opts.region, &regions);
        let baggage = calc_baggage_cost();
        let (transport_cost, transport_time) = calc_transport_cost(&hotel_hub, &itinerary, &opts.region, opts.pax, &regions, opts.jpy_rate);
        let surcharge = js_round((baggage + transport_cost) as f64 / opts.pax as f64);
        let true_total = o.price_per_person + surcharge;
        results.push(TrueCostOffer {
            file: o.file.clone(),
            source_name: o.source_id.clone(),
            price_per_person: o.price_per_person,
            currency: o.currency.clone(),
            airline: o.airline.clone(),
            hotel: o.hotel.clone(),
            hotel_area_type: area_type,
            baggage_cost: baggage,
            transport_cost,
            transport_time_min: transport_time,
            true_total,
            breakdown: format!("pkg:{} bag:{} xport:{}", o.price_per_person, baggage, transport_cost),
        });
    }
    results.sort_by_key(|r| r.true_total);

    print_results(&results, opts);
    Ok(())
}

struct RawOffer {
    file: String,
    source_id: String,
    price_per_person: i64,
    currency: String,
    airline: String,
    hotel: String,
}

struct TrueCostOffer {
    file: String,
    source_name: String,
    price_per_person: i64,
    currency: String,
    airline: String,
    hotel: String,
    hotel_area_type: String,
    baggage_cost: i64,
    transport_cost: i64,
    transport_time_min: i64,
    true_total: i64,
    breakdown: String,
}

fn parse_itinerary(spec: Option<&str>) -> Vec<(String, i64)> {
    let mut out = Vec::new();
    if let Some(s) = spec {
        for part in s.split(',') {
            let p = part.trim();
            #[allow(clippy::collapsible_if)]
            if let Some((dest, days)) = p.split_once(':') {
                let dest = dest.trim();
                if !dest.is_empty() {
                    if let Ok(d) = days.parse::<i64>() {
                        out.push((dest.to_string(), d));
                    }
                }
            }
        }
    }
    out
}

fn detect_hotel_area(hotel: &str, region: &str, hotel_areas: &BTreeMap<String, BTreeMap<String, Vec<String>>>) -> String {
    if let Some(areas) = hotel_areas.get(region) {
        for (area_type, keywords) in areas {
            for kw in keywords {
                if hotel.contains(kw.as_str()) {
                    return area_type.clone();
                }
            }
        }
    }
    "unknown".to_string()
}

fn area_type_to_hub(area_type: &str, region: &str, regions: &BTreeMap<String, RegionData>) -> String {
    let Some(rd) = regions.get(region) else { return String::new() };
    for (hub_id, hub_type, _) in &rd.hubs {
        if hub_type == area_type {
            return hub_id.clone();
        }
    }
    for (hub_id, hub_type, _) in &rd.hubs {
        if hub_type == "central" {
            return hub_id.clone();
        }
    }
    String::new()
}

fn calc_baggage_cost() -> i64 {
    // FIT packages (the only kind scanOffers emits) include baggage → 0.
    0
}

fn lookup_route<'a>(from: &str, to: &str, routes: &'a BTreeMap<String, TransportRoute>) -> Option<&'a TransportRoute> {
    routes.get(&format!("{from}-{to}")).or_else(|| routes.get(&format!("{to}-{from}")))
}

fn calc_transport_cost(hotel_hub: &str, itinerary: &[(String, i64)], region: &str, pax: i64, regions: &BTreeMap<String, RegionData>, jpy2twd: f64) -> (i64, i64) {
    let Some(rd) = regions.get(region) else { return (0, 0) };
    if hotel_hub.is_empty() {
        return (0, 0);
    }
    let mut total_jpy = 0_i64;
    let mut total_min = 0_i64;
    // airport transfer: 2 trips
    #[allow(clippy::collapsible_if)]
    if let Some((airport_hub, _, _)) = rd.hubs.iter().find(|(_, t, _)| t == "airport") {
        if let Some(rt) = lookup_route(airport_hub, hotel_hub, &rd.routes) {
            total_jpy += rt.cost_jpy * pax * 2;
            total_min += rt.time_min * 2;
        }
    }
    for (dest, days) in itinerary {
        if let Some(rt) = lookup_route(hotel_hub, dest, &rd.routes) {
            total_jpy += rt.cost_jpy * pax * 2 * days;
            total_min += rt.time_min * 2 * days;
        }
    }
    (js_round(total_jpy as f64 * jpy2twd), total_min)
}

fn print_results(results: &[TrueCostOffer], opts: &TrueCostArgs) {
    // best_time = Math.min over transport_time_min (a real number, addition works).
    let best_time = results.iter().map(|r| r.transport_time_min).min().unwrap_or(0);

    println!("\n╔════════════════════════════════════════════════════════════════════════════════════╗");
    println!("║  TRUE COST COMPARISON: {}║", pad_end(&opts.region.to_uppercase(), 57));
    println!("╚════════════════════════════════════════════════════════════════════════════════════╝");
    println!("  Pax: {} | Date: {} | JPY rate: {}", opts.pax, opts.date.as_deref().unwrap_or("(best)"), fmt_rate(opts.jpy_rate));
    if let Some(it) = &opts.itinerary {
        println!("  Itinerary: {it}");
    }
    println!();

    println!("┌──────────┬──────────┬─────────┬───────────┬────────────┬──────────┬──────────────────────┐");
    println!("│ Source   │ Package  │ Baggage │ Transport │ TRUE/person│ Time     │ Hotel                │");
    println!("├──────────┼──────────┼─────────┼───────────┼────────────┼──────────┼──────────────────────┤");
    for r in results {
        let src = pad_end(&truncate(&r.source_name, 8), 8);
        let pkg = pad_start(&locale(r.price_per_person), 8);
        let bag = if r.baggage_cost > 0 { pad_start(&locale(r.baggage_cost), 7) } else { "      0".to_string() };
        let xport = if r.transport_cost > 0 { pad_start(&locale(r.transport_cost), 9) } else { "        0".to_string() };
        let total = pad_start(&locale(r.true_total), 10);
        let time_diff = r.transport_time_min - best_time;
        let time_str = if time_diff == 0 { "optimal ".to_string() } else { pad_end(&format!("+{time_diff}min"), 8) };
        let hotel = pad_end(&truncate(if r.hotel.is_empty() { "-" } else { &r.hotel }, 20), 20);
        println!("│ {} │ {} │ {} │ {} │ {} │ {} │ {} │", src, pkg, bag, xport, total, time_str, hotel);
    }
    println!("└──────────┴──────────┴─────────┴───────────┴────────────┴──────────┴──────────────────────┘");

    if let Some(best) = results.first() {
        println!("\n💡 Best true value: {} at {} {}/person", best.source_name, best.currency, locale(best.true_total));
        println!("   {}", best.breakdown);
        if !best.hotel.is_empty() {
            println!("   Hotel: {} ({})", best.hotel, best.hotel_area_type);
        }
        if !best.airline.is_empty() {
            println!("   Airline: {}", best.airline);
        }
        // cheapest by package price (TS: [...results].sort((a,b)=>a.ppp-b.ppp)[0])
        let cheapest_pkg = results.iter().min_by_key(|r| r.price_per_person).unwrap();
        if cheapest_pkg.file != best.file {
            println!("\n⚠️  Cheapest package ({} {}) is NOT the best true value!", cheapest_pkg.source_name, locale(cheapest_pkg.price_per_person));
            println!("   Hidden costs: baggage {} + transport {}", locale(cheapest_pkg.baggage_cost), locale(cheapest_pkg.transport_cost));
        }
    }
    println!();
}

// Match JS number formatting of the rate (e.g. 0.22 → "0.22").
fn fmt_rate(r: f64) -> String {
    format!("{r}")
}
