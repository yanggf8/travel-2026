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

struct FreshnessResult {
    age_hours: Option<f64>,
    offer_count: i64,
    recommendation: &'static str,
    region: Option<String>,
}

pub async fn run(opts: &FreshnessArgs) -> Result<(), String> {
    use travel_db::repo::{freshness, offers::OfferFilter};

    let conn = db::connect_read().await?;
    let counted = if let (Some(plan_id), Some(dest)) = (&opts.plan_id, &opts.destination) {
        // Plan-based path: plan_offer_provenance.
        freshness::plan_provenance_freshness(&conn, plan_id, dest, &opts.source).await?
    } else {
        // Legacy path: offers table (source_id is always present, plus optional region/dates).
        let mut filter = OfferFilter::new().source_id(&opts.source);
        if let Some(r) = &opts.region {
            filter = filter.region(r);
        }
        if let Some(s) = &opts.start {
            filter = filter.departure_from(s);
        }
        if let Some(e) = &opts.end {
            filter = filter.departure_to(e);
        }
        freshness::offers_freshness(&conn, filter.build()).await?
    };

    let result = compute(
        counted.count,
        counted.newest.as_deref(),
        opts.max_age_hours,
        opts.region.clone(),
    );

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
