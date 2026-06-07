// `travel check-freshness` — check data freshness for an OTA source in Turso.
// Read-only, plain-text. Ports checkFreshness from src/services/turso-service.ts
// and the check-freshness handler in src/cli/commands/turso.ts.
//
// Two query paths (matching TS):
//   - plan-based: when both --plan-id and --dest given → plan_offer_provenance
//   - legacy:     otherwise → offers table (filter by region/start/end)

use crate::db;
use chrono::{NaiveDateTime, Utc};

pub struct FreshnessArgs {
    pub source: String,
    pub region: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub max_age_hours: f64,
    pub plan_id: Option<String>,
    pub destination: Option<String>,
}

impl FreshnessArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut source: Option<String> = None;
        let mut region = None;
        let mut start = None;
        let mut end = None;
        let mut max_age_hours = 24.0;
        let mut plan_id = std::env::var("TRAVEL_PLAN_ID").ok().filter(|s| !s.is_empty());
        let mut destination = None;

        let mut i = 0;
        while i < args.len() {
            let key = args[i].as_str();
            let val = || {
                args.get(i + 1)
                    .cloned()
                    .ok_or_else(|| format!("{key} requires a value"))
            };
            match key {
                "--source" => source = Some(val()?),
                "--region" => region = Some(val()?),
                "--start" => start = Some(val()?),
                "--end" => end = Some(val()?),
                "--max-age" => {
                    max_age_hours = val()?
                        .parse()
                        .map_err(|_| "--max-age must be a number".to_string())?
                }
                "--plan-id" => plan_id = Some(val()?),
                "--dest" | "--destination" => destination = Some(val()?),
                other => return Err(format!("unknown flag for check-freshness: {other}")),
            }
            i += 2;
        }

        let source = source.ok_or_else(|| {
            "Error: check-freshness requires --source <id>\n\
             Example: check-freshness --source besttour --region kansai"
                .to_string()
        })?;

        Ok(FreshnessArgs {
            source,
            region,
            start,
            end,
            max_age_hours,
            plan_id,
            destination,
        })
    }
}

fn sql_quote(v: &str) -> String {
    v.replace('\'', "''")
}

struct FreshnessResult {
    age_hours: Option<f64>,
    offer_count: i64,
    recommendation: &'static str,
    region: Option<String>,
}

pub async fn run(opts: &FreshnessArgs) -> Result<(), String> {
    let sql = if let (Some(plan_id), Some(dest)) = (&opts.plan_id, &opts.destination) {
        // Plan-based path: plan_offer_provenance.
        format!(
            "SELECT MAX(scraped_at) as newest, SUM(COALESCE(offer_count, 0)) as cnt \
             FROM plan_offer_provenance \
             WHERE plan_id = '{}' AND destination = '{}' AND source_id = '{}'",
            sql_quote(plan_id),
            sql_quote(dest),
            sql_quote(&opts.source),
        )
    } else {
        // Legacy path: offers table.
        let mut conds: Vec<String> = vec![format!("source_id = '{}'", sql_quote(&opts.source))];
        if let Some(r) = &opts.region {
            conds.push(format!("region = '{}'", sql_quote(r)));
        }
        if let Some(s) = &opts.start {
            conds.push(format!("departure_date >= '{}'", sql_quote(s)));
        }
        if let Some(e) = &opts.end {
            conds.push(format!("departure_date <= '{}'", sql_quote(e)));
        }
        format!(
            "SELECT COUNT(*) as cnt, MAX(scraped_at) as newest FROM offers WHERE {}",
            conds.join(" AND ")
        )
    };

    let conn = db::connect_read().await?;
    let mut rows = conn
        .query(sql.as_str(), ())
        .await
        .map_err(|err| format!("failed to query freshness from Turso: {err}"))?;

    // Pull cnt + newest by column name regardless of SELECT order.
    let (mut count, mut newest): (i64, Option<String>) = (0, None);
    if let Some(row) = rows
        .next()
        .await
        .map_err(|err| format!("failed to read freshness row: {err}"))?
    {
        // Both paths alias the columns `cnt` and `newest`.
        count = column_i64(&row, "cnt").unwrap_or(0);
        newest = column_string(&row, "newest").filter(|s| !s.is_empty());
    }

    let result = compute(count, newest.as_deref(), opts.max_age_hours, opts.region.clone());

    println!("Source:  {}", opts.source);
    if let Some(r) = &result.region {
        println!("Region:  {r}");
    }
    println!("Result:  {}", result.recommendation);
    if let Some(age) = result.age_hours {
        println!("  Age:     {:.1}h", age);
    }
    println!("  Offers:  {}", result.offer_count);
    Ok(())
}

fn compute(
    count: i64,
    newest: Option<&str>,
    max_age_hours: f64,
    region: Option<String>,
) -> FreshnessResult {
    let Some(newest) = newest else {
        return FreshnessResult {
            age_hours: None,
            offer_count: 0,
            recommendation: "no_data",
            region,
        };
    };
    if count == 0 {
        return FreshnessResult {
            age_hours: None,
            offer_count: 0,
            recommendation: "no_data",
            region,
        };
    }

    match parse_scraped_at(newest) {
        Some(ts_ms) => {
            let now_ms = Utc::now().timestamp_millis();
            let age_hours = (now_ms - ts_ms) as f64 / (1000.0 * 60.0 * 60.0);
            let fresh = age_hours <= max_age_hours;
            FreshnessResult {
                age_hours: Some(age_hours),
                offer_count: count,
                recommendation: if fresh { "skip" } else { "rescrape" },
                region,
            }
        }
        // Matches the TS NaN guard: malformed scraped_at → rescrape, age unknown.
        None => FreshnessResult {
            age_hours: None,
            offer_count: count,
            recommendation: "rescrape",
            region,
        },
    }
}

/// Parse a DB `scraped_at` value as UTC milliseconds. Mirrors the TS
/// `new Date(newest + (includes 'Z' ? '' : 'Z'))` behavior: bare timestamps are
/// treated as UTC. Returns None on malformed input (TS NaN path).
fn parse_scraped_at(raw: &str) -> Option<i64> {
    // Already has explicit zone / Z → let chrono's RFC3339 handle it; on failure
    // fall through to bare-format parsing below.
    #[allow(clippy::collapsible_if)]
    if raw.contains('Z') || raw.contains('+') {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) {
            return Some(dt.timestamp_millis());
        }
    }
    // Bare "YYYY-MM-DDTHH:MM:SS[.fff]" or "YYYY-MM-DD HH:MM:SS" → assume UTC.
    let normalized = raw.replace(' ', "T");
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(&normalized, fmt) {
            return Some(dt.and_utc().timestamp_millis());
        }
    }
    None
}

fn column_i64(row: &libsql::Row, name: &str) -> Option<i64> {
    let idx = column_index(row, name)?;
    match row.get_value(idx).ok()? {
        libsql::Value::Integer(n) => Some(n),
        libsql::Value::Real(f) => Some(f as i64),
        libsql::Value::Text(s) => s.parse().ok(),
        _ => None,
    }
}

fn column_string(row: &libsql::Row, name: &str) -> Option<String> {
    let idx = column_index(row, name)?;
    match row.get_value(idx).ok()? {
        libsql::Value::Text(s) => Some(s),
        _ => None,
    }
}

fn column_index(row: &libsql::Row, name: &str) -> Option<i32> {
    let n = row.column_count();
    (0..n).find(|&i| row.column_name(i) == Some(name))
}
