// `travel db query-offers` — read the raw `offers` table with trip-criteria
// filters. Ports scripts/turso-query-offers.ts. Read-only, plain text.
//
// Differences from the TS original (intentional, per the migration plan):
//   - `--json` flag removed; the migration removes user-facing JSON output
//     in the root CLI. Plain table is the only output mode.
//   - Null values render as `-` or empty (the TS unwrapTursoCell path leaked
//     `{"type":"null"}` strings when the libsql cell wrapper slipped through;
//     this port cleans that up).
//   - Filters and column list are the same as TS.

use crate::db;
use libsql::Value;
use std::process;

const DEFAULT_LIMIT: i64 = 50;
const NAME_TRUNCATE: usize = 40;
const AGE_TRUNCATE: usize = 6;

#[derive(Default, Debug)]
pub struct QueryOffersArgs {
    pub destination: Option<String>,
    pub region: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub sources: Option<String>,
    pub kind: Option<String>,
    pub max_price: Option<i64>,
    pub fresh_hours: Option<i64>,
    pub limit: i64,
    pub include_undated: bool,
    pub show_sql: bool,
    pub capture_id: Option<String>,
    pub job_id: Option<String>,
    pub attempt_id: Option<String>,
}

impl QueryOffersArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut o = QueryOffersArgs {
            limit: DEFAULT_LIMIT,
            ..Default::default()
        };
        let mut i = 0;
        while i < args.len() {
            let key = args[i].as_str();
            let val = || {
                args.get(i + 1)
                    .cloned()
                    .ok_or_else(|| format!("{key} requires a value"))
            };
            match key {
                "--destination" | "--dest" => o.destination = Some(val()?),
                "--region" => o.region = Some(val()?),
                "--start" => o.start = Some(val()?),
                "--end" => o.end = Some(val()?),
                "--sources" => o.sources = Some(val()?),
                "--type" => o.kind = Some(val()?),
                "--max-price" => {
                    o.max_price = Some(
                        val()?
                            .parse()
                            .map_err(|_| "--max-price must be an integer".to_string())?,
                    )
                }
                "--fresh-hours" => {
                    o.fresh_hours = Some(
                        val()?
                            .parse()
                            .map_err(|_| "--fresh-hours must be an integer".to_string())?,
                    )
                }
                "--capture-id" => o.capture_id = Some(val()?),
                "--job-id" => o.job_id = Some(val()?),
                "--attempt-id" => o.attempt_id = Some(val()?),
                "--max" => {
                    o.limit = val()?
                        .parse()
                        .map_err(|_| "--max must be an integer".to_string())?
                }
                "--include-undated" => {
                    o.include_undated = true;
                    i += 1;
                    continue;
                }
                "--sql" => {
                    o.show_sql = true;
                    i += 1;
                    continue;
                }
                "--help" | "-h" => {
                    print_usage();
                    process::exit(0);
                }
                other => return Err(format!("unknown flag for query-offers: {other}")),
            }
            i += 2;
        }
        validate(&o)?;
        Ok(o)
    }
}

fn validate(o: &QueryOffersArgs) -> Result<(), String> {
    if let Some(s) = &o.start
        && !is_iso_date(s)
    {
        return Err(format!("Invalid --start date format: {s} (expected YYYY-MM-DD)"));
    }
    if let Some(e) = &o.end
        && !is_iso_date(e)
    {
        return Err(format!("Invalid --end date format: {e} (expected YYYY-MM-DD)"));
    }
    if let (Some(s), Some(e)) = (&o.start, &o.end)
        && s > e
    {
        return Err(format!("Invalid date range: --start {s} is after --end {e}"));
    }
    if let Some(t) = &o.kind
        && !matches!(t.as_str(), "package" | "flight" | "hotel")
    {
        return Err(format!("Invalid --type: {t} (expected package|flight|hotel)"));
    }
    if let Some(p) = o.max_price
        && p < 0
    {
        return Err(format!("Invalid --max-price: {p} (must be >= 0)"));
    }
    if let Some(f) = o.fresh_hours
        && f < 0
    {
        return Err(format!("Invalid --fresh-hours: {f} (must be >= 0)"));
    }
    if o.limit <= 0 {
        return Err(format!("Invalid --max: {} (must be > 0)", o.limit));
    }
    Ok(())
}

fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[8..10].iter().all(u8::is_ascii_digit)
}

/// Build the parameterized WHERE fragment + bound params via the shared DAL builder
/// (same predicate order as before, so placeholder numbering is deterministic).
fn build_where(o: &QueryOffersArgs) -> travel_db::repo::offers::OfferWhere {
    let mut filter = travel_db::repo::offers::OfferFilter::new();
    if let Some(d) = &o.destination {
        filter = filter.destination(d);
    }
    if let Some(r) = &o.region {
        filter = filter.region(r);
    }
    filter = filter.departure_window(o.start.as_deref(), o.end.as_deref(), o.include_undated);
    if let Some(csv) = &o.sources {
        filter = filter.source_id_in_csv(csv);
    }
    if let Some(t) = &o.kind {
        filter = filter.offer_type(t);
    }
    if let Some(p) = o.max_price {
        filter = filter.max_price(p);
    }
    if let Some(f) = o.fresh_hours {
        filter = filter.fresh_within_hours(f);
    }
    if let Some(c) = &o.capture_id {
        filter = filter.capture_id(c);
    }
    if let Some(j) = &o.job_id {
        filter = filter.produced_by_job_id(j);
    }
    if let Some(a) = &o.attempt_id {
        filter = filter.produced_by_attempt_id(a);
    }
    filter.build()
}

/// Build the full SQL string + its bound params (placeholders in `?N` form).
fn build_sql(o: &QueryOffersArgs) -> (String, Vec<libsql::Value>) {
    let where_built = build_where(o);
    // Single-line SQL to match TS .trim().replace(/\s+/g, ' ') behavior.
    let mut sql = String::from(
        "SELECT id, source_id, type, name, price_per_person, currency, \
         departure_date, return_date, airline, hotel_name, scraped_at, \
         capture_id, produced_by_job_id, produced_by_attempt_id, \
         (julianday('now') - julianday(COALESCE(last_seen_at, scraped_at))) * 24.0 AS age_hours \
         FROM offers",
    );
    if !where_built.clause.is_empty() {
        sql.push(' ');
        sql.push_str(&where_built.clause);
    }
    sql.push_str(
        " ORDER BY \
         CASE WHEN price_per_person IS NULL THEN 1 ELSE 0 END, \
         price_per_person ASC, \
         COALESCE(last_seen_at, scraped_at) DESC \
         LIMIT ",
    );
    sql.push_str(&o.limit.to_string());
    (sql, where_built.params)
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Integer(n) => n.to_string(),
        Value::Real(f) => f.to_string(),
        Value::Text(s) => s.clone(),
        Value::Blob(b) => format!("<blob {} bytes>", b.len()),
    }
}

/// Render one bound param for the `--sql` debug line (quotes text, bare ints).
fn render_param(v: &Value) -> String {
    match v {
        Value::Text(s) => format!("'{s}'"),
        other => value_to_string(other),
    }
}

fn dash(v: &str) -> &str {
    if v.is_empty() { "-" } else { v }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn format_age(scraped_at: &str) -> String {
    if scraped_at.is_empty() {
        return "?".to_string();
    }
    // Parse "YYYY-MM-DDTHH:MM:SS[.fff][Z|+hh:mm]". For freshness the
    // precision beyond hours doesn't matter, so we accept several shapes.
    let ts = parse_iso_to_ms(scraped_at);
    let Some(ts) = ts else {
        return "?".to_string();
    };
    let now = chrono::Utc::now().timestamp_millis();
    let hours = ((now - ts) as f64) / (1000.0 * 60.0 * 60.0);
    let hours = hours.round() as i64;
    if hours < 1 {
        "<1h".to_string()
    } else if hours < 24 {
        format!("{hours}h")
    } else {
        format!("{}d", hours / 24)
    }
}

fn parse_iso_to_ms(raw: &str) -> Option<i64> {
    use chrono::NaiveDateTime;
    if (raw.contains('Z') || raw.contains('+'))
        && let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw)
    {
        return Some(dt.timestamp_millis());
    }
    let normalized = raw.replace(' ', "T");
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(&normalized, fmt) {
            return Some(dt.and_utc().timestamp_millis());
        }
    }
    None
}

pub async fn run(opts: &QueryOffersArgs) -> Result<(), String> {
    let (sql, params) = build_sql(opts);
    if opts.show_sql {
        // Parameterized SQL: values are bound (?N), not inlined. Show both for debugging.
        println!("SQL: {sql}");
        if !params.is_empty() {
            let rendered: Vec<String> = params.iter().map(render_param).collect();
            println!("PARAMS: [{}]", rendered.join(", "));
        }
        println!();
    }

    let conn = db::connect_read().await?;
    let mut rows = conn
        .query(&sql, params)
        .await
        .map_err(|err| format!("failed to query offers from Turso: {err}"))?;

    let mut collected: Vec<[String; 15]> = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|err| format!("failed to read offer row: {err}"))?
    {
        let mut vals: [String; 15] = std::array::from_fn(|_| String::new());
        for (i, slot) in vals.iter_mut().enumerate() {
            *slot = row
                .get_value(i as i32)
                .map(|v| value_to_string(&v))
                .unwrap_or_default();
        }
        collected.push(vals);
    }

    if collected.is_empty() {
        println!("No offers found matching criteria.\n");
        print_applied_filters(opts);
        return Ok(());
    }

    print_table(&collected, opts.limit);
    Ok(())
}

fn print_table(rows: &[[String; 15]], limit: i64) {
    let source_w = 12usize;
    let type_w = 9usize;
    let price_w = 10usize;
    let date_w = 23usize;
    let age_w = AGE_TRUNCATE;
    let prov_w = 36usize;

    println!("Found {} offer(s):\n", rows.len());
    let header = format!(
        "{:<sw$} {:<tw$} {:<pw$} {:<dw$} {:<aw$} {:<pv$} {:<pv$} {:<pv$} {}",
        "SOURCE",
        "TYPE",
        "PRICE",
        "DATE",
        "AGE",
        "CAPTURE",
        "JOB",
        "ATTEMPT",
        "NAME",
        sw = source_w,
        tw = type_w,
        pw = price_w,
        dw = date_w,
        aw = age_w,
        pv = prov_w,
    );
    println!("{header}");
    println!("{}", "-".repeat(header.len()));

    for r in rows {
        let source = dash(&r[1]);
        let kind = dash(&r[2]);
        let price_num = r[4].parse::<f64>().ok();
        let currency = if r[5].is_empty() { "TWD" } else { &r[5] };
        let price_str = match price_num {
            Some(p) => format!("{currency} {p:.0}"),
            None => "—".to_string(),
        };
        let depart = &r[6];
        let ret = &r[7];
        let date_str = if depart.is_empty() {
            "—".to_string()
        } else if ret.is_empty() {
            depart.clone()
        } else {
            format!("{depart}→{ret}")
        };
        let age = format_age(&r[10]);
        let prov = |s: &str| {
            let d = dash(s);
            if d.chars().count() >= prov_w { d.to_string() } else { format!("{d:<prov_w$}") }
        };
        let capture = prov(&r[11]);
        let job = prov(&r[12]);
        let attempt = prov(&r[13]);
        let name = truncate(dash(&r[3]), NAME_TRUNCATE);
        println!(
            "{:<sw$} {:<tw$} {:<pw$} {:<dw$} {:<aw$} {:<pv$} {:<pv$} {:<pv$} {}",
            source,
            kind,
            price_str,
            date_str,
            age,
            capture,
            job,
            attempt,
            name,
            sw = source_w,
            tw = type_w,
            pw = price_w,
            dw = date_w,
            aw = age_w,
            pv = prov_w,
        );
    }
    println!();
    println!("Showing {} of max {} results.", rows.len(), limit);
}

fn print_applied_filters(o: &QueryOffersArgs) {
    println!("Filters applied:");
    if let Some(d) = &o.destination {
        println!("  destination: {d}");
    }
    if let Some(r) = &o.region {
        println!("  region: {r}");
    }
    if o.start.is_some() || o.end.is_some() {
        let s = o.start.as_deref().unwrap_or("*");
        let e = o.end.as_deref().unwrap_or("*");
        println!("  dates: {s} to {e}");
    }
    if let Some(s) = &o.sources {
        println!("  sources: {s}");
    }
    if let Some(p) = o.max_price {
        println!("  max_price: {p}");
    }
    if let Some(f) = o.fresh_hours {
        println!("  fresh_hours: {f}");
    }
    if let Some(c) = &o.capture_id {
        println!("  capture_id: {c}");
    }
    if let Some(j) = &o.job_id {
        println!("  job_id: {j}");
    }
    if let Some(a) = &o.attempt_id {
        println!("  attempt_id: {a}");
    }
}

fn print_usage() {
    println!(
        "Query Turso for offers matching trip criteria.\n\
         \n\
         Usage:\n  \
           travel db query-offers --destination osaka_2026 --start 2026-02-24 --end 2026-02-28\n  \
           travel db query-offers --region kansai --sources besttour,liontravel --max 10\n  \
           travel db query-offers --fresh-hours 24 --max 20\n\
         \n\
         Options:\n  \
           --destination <slug>     Filter by destination (e.g. osaka_2026, tokyo_2026)\n  \
           --region <name>          Filter by region (e.g. kansai, tokyo)\n  \
           --start <YYYY-MM-DD>     Filter departure_date >= start\n  \
           --end <YYYY-MM-DD>       Filter departure_date <= end\n  \
           --sources <csv>          Filter by source_id (comma-separated)\n  \
           --type <type>            Filter by type (package, flight, hotel)\n  \
           --max-price <int>        Filter price_per_person <= max\n  \
           --fresh-hours <int>      Only offers scraped within N hours\n  \
           --max <int>              Limit results (default: 50)\n  \
           --include-undated        When filtering by date, also include offers without departure_date\n  \
           --capture-id <id>        Filter by capture_id (exact match)\n  \
           --job-id <id>            Filter by produced_by_job_id (exact match)\n  \
           --attempt-id <id>        Filter by produced_by_attempt_id (exact match)\n  \
           --sql                    Show generated SQL (for debugging)\n\
         \n\
         Output columns:\n  \
           SOURCE, TYPE, PRICE, DATE, AGE, CAPTURE, JOB, ATTEMPT, NAME"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn param_texts(params: &[Value]) -> Vec<String> {
        params
            .iter()
            .map(|v| match v {
                Value::Text(s) => s.clone(),
                Value::Integer(n) => n.to_string(),
                _ => String::new(),
            })
            .collect()
    }

    #[test]
    fn build_where_empty_when_no_filters() {
        let w = build_where(&QueryOffersArgs {
            limit: DEFAULT_LIMIT,
            ..Default::default()
        });
        assert_eq!(w.clause, "");
        assert!(w.params.is_empty());
    }

    #[test]
    fn build_where_orders_predicates_and_binds_values() {
        let o = QueryOffersArgs {
            destination: Some("osaka_2026".to_string()),
            region: Some("kansai".to_string()),
            start: Some("2026-02-24".to_string()),
            end: Some("2026-02-28".to_string()),
            sources: Some("besttour, liontravel".to_string()),
            kind: Some("package".to_string()),
            max_price: Some(40000),
            fresh_hours: Some(24),
            limit: 10,
            include_undated: false,
            show_sql: false,
            capture_id: None,
            job_id: None,
            attempt_id: None,
        };
        let w = build_where(&o);
        assert_eq!(
            w.clause,
            "WHERE destination = ?1 AND region = ?2 AND departure_date IS NOT NULL \
             AND departure_date >= ?3 AND departure_date <= ?4 \
             AND source_id IN (?5,?6) AND type = ?7 AND price_per_person <= ?8 \
             AND COALESCE(last_seen_at, scraped_at) IS NOT NULL \
             AND julianday(COALESCE(last_seen_at, scraped_at)) >= (julianday('now') - (?9 / 24.0))"
        );
        assert_eq!(
            param_texts(&w.params),
            vec![
                "osaka_2026",
                "kansai",
                "2026-02-24",
                "2026-02-28",
                "besttour",
                "liontravel",
                "package",
                "40000",
                "24",
            ]
        );
    }

    #[test]
    fn build_where_include_undated_widens_bounds() {
        let o = QueryOffersArgs {
            start: Some("2026-09-01".to_string()),
            include_undated: true,
            limit: DEFAULT_LIMIT,
            ..Default::default()
        };
        let w = build_where(&o);
        assert_eq!(w.clause, "WHERE (departure_date IS NULL OR departure_date >= ?1)");
        assert_eq!(param_texts(&w.params), vec!["2026-09-01"]);
    }

    #[test]
    fn build_sql_wraps_where_with_select_order_limit() {
        let o = QueryOffersArgs {
            region: Some("kansai".to_string()),
            limit: 7,
            ..Default::default()
        };
        let (sql, params) = build_sql(&o);
        assert!(sql.starts_with("SELECT id, source_id, type, name, price_per_person"));
        assert!(sql.contains("FROM offers WHERE region = ?1 ORDER BY"));
        assert!(sql.trim_end().ends_with("LIMIT 7"));
        assert_eq!(param_texts(&params), vec!["kansai"]);
        assert!(sql.contains("capture_id, produced_by_job_id, produced_by_attempt_id"));
    }

    #[test]
    fn build_where_provenance_filters_bind_exact_ids() {
        let o = QueryOffersArgs {
            capture_id: Some("cap-1".to_string()),
            job_id: Some("job-1".to_string()),
            attempt_id: Some("att-1".to_string()),
            limit: DEFAULT_LIMIT,
            ..Default::default()
        };
        let w = build_where(&o);
        assert_eq!(
            w.clause,
            "WHERE capture_id = ?1 AND produced_by_job_id = ?2 AND produced_by_attempt_id = ?3"
        );
        assert_eq!(param_texts(&w.params), vec!["cap-1", "job-1", "att-1"]);
    }

    #[test]
    fn parse_rejects_unknown_flag() {
        let err = QueryOffersArgs::parse(&["--totally-bogus".to_string()]).unwrap_err();
        assert!(err.contains("unknown flag"), "err={err}");
    }

    #[test]
    fn parse_provenance_flags_after_sql_boolean() {
        // `--sql` is boolean (i+=1); a following value-flag must still bind.
        let o = QueryOffersArgs::parse(&[
            "--sql".to_string(),
            "--capture-id".to_string(),
            "c1".to_string(),
            "--job-id".to_string(),
            "j1".to_string(),
            "--attempt-id".to_string(),
            "a1".to_string(),
        ])
        .unwrap();
        assert!(o.show_sql);
        assert_eq!(o.capture_id.as_deref(), Some("c1"));
        assert_eq!(o.job_id.as_deref(), Some("j1"));
        assert_eq!(o.attempt_id.as_deref(), Some("a1"));
    }

    #[test]
    fn parse_include_undated_does_not_swallow_next_flag() {
        // `--include-undated` is boolean (i+=1); a following value-flag must still bind.
        let o = QueryOffersArgs::parse(&[
            "--include-undated".to_string(),
            "--capture-id".to_string(),
            "c1".to_string(),
        ])
        .unwrap();
        assert!(o.include_undated);
        assert_eq!(o.capture_id.as_deref(), Some("c1"));
    }
}
