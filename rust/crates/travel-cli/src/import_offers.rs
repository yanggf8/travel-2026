// `travel import-offers [--dest <slug>] [--dir <path>] [--files <csv>] [--start <date>] [--end <date>] [--pax N] [--note <text>] [--dry-run]`
//
// Port of the TS importOffersCommand + StateManager.importScrapedOffers() +
// importPackageOffers() + saveWithTracking('import-offers', ...) path.
//
// Reads scrape JSON files, parses them via scrape_parser, and writes
// normalized rows to Turso DB. Merge-by-offer-id semantics: re-importing
// source A does not drop source B's offers.
//
// CLI output must match byte-for-byte:
//   "Importing offers for <dest> from <N> file(s)[ (dry-run)]..."
//   per source: "  <source>: <count> offer(s)"
//   "Saved to Turso (plan_offers)."  (non-dry-run, counts > 0)
//   "No offers imported (no matching files or all offers filtered out)."  (no counts)
//   stderr + exit 1: "No JSON files found. Use --dir <path> or --files <csv>."
//
// Volatile fields (normalized out of the diff): updated_at, run_id,
// started_at/completed_at, event_at.
//
// ⚠️ TS PARITY DIVERGENCE (intentional — see memory ts-import-offers-broken):
// The TS `import-offers` path is BROKEN end-to-end and writes nothing. The TS
// `parseScrapeFiles()` emits a camelCase `CanonicalOffer` (`sourceId`,
// `pricePerPerson`, `flight.outbound.departureCode`, …) and stores it verbatim
// into `results.offers`, but the plan-level `OfferSchema` (src/state/schemas.ts)
// requires snake_case (`source_id`, `price_per_person`,
// `flight.outbound.departure_airport_code`, `date_pricing[date].price`). On
// `save()` the Zod validation THROWS for every offer, so the TS command prints
// only the header line, leaves a half-finished `operation_runs` row
// (status='started', version_after=NULL), and imports zero offers. Even absent
// validation, `syncNormalizedTables()` reads snake_case off the camelCase object
// → all-NULL columns.
//
// This Rust port reproduces the INTENDED write surface documented in the porting
// task (snake_case normalized rows from the parsed offer), NOT the TS crash —
// the Rust parser yields a typed `CanonicalOffer` struct and this writer maps its
// fields straight to the correct snake_case columns. A literal TS-vs-Rust DB
// snapshot diff therefore DIFFERS by design (TS=nothing, Rust=full correct delta).
// Do NOT "fix" this to match the TS crash.

use crate::cascade::common::{
    insert_event, insert_kv_rows, new_run_id, next_dest_process_sort_order,
    next_timeline_sort_order, now_db_datetime, now_rfc3339, read_version,
};
use crate::scrape_parser;
use crate::scrape_parser::CanonicalOffer;
use libsql::Connection;
use std::path::Path;

const P34: &str = "process_3_4_packages";

pub struct ImportOpts {
    pub dest: String,
    pub dir: Option<String>,
    pub files: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub pax: i64,
    pub note: Option<String>,
    pub dry_run: bool,
    pub plan_id: String,
}

/// Parse CLI args for import-offers.
pub fn parse_args(rest: &[String]) -> Result<ImportOpts, String> {
    if rest.iter().any(|a| a == "--help" || a == "-h") {
        println!("Usage:\n  travel import-offers [--dest <slug>] [--dir <path>] [--files <csv>] [--start <date>] [--end <date>] [--pax N] [--note <text>] [--dry-run]");
        std::process::exit(0);
    }
    let mut dest = None;
    let mut dir = None;
    let mut files = None;
    let mut start = None;
    let mut end = None;
    let mut pax = None;
    let mut note = None;
    let mut dry_run = false;

    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--dest" => {
                i += 1;
                dest = rest.get(i).cloned();
            }
            "--dir" => {
                i += 1;
                dir = rest.get(i).cloned();
            }
            "--files" => {
                i += 1;
                files = rest.get(i).cloned();
            }
            "--start" => {
                i += 1;
                start = rest.get(i).cloned();
            }
            "--end" => {
                i += 1;
                end = rest.get(i).cloned();
            }
            "--pax" => {
                i += 1;
                pax = rest.get(i).and_then(|s| s.parse::<i64>().ok());
            }
            "--note" => {
                i += 1;
                note = rest.get(i).cloned();
            }
            "--dry-run" => {
                dry_run = true;
            }
            _ => {}
        }
        i += 1;
    }

    let plan_id = std::env::var("TRAVEL_PLAN_ID")
        .unwrap_or_else(|_| "test-set-dates-2026".to_string());

    // Resolve destination: --dest wins, else active_destination from DB
    let dest_slug = dest.unwrap_or_else(|| {
        // Will resolve later from plan_metadata
        String::new()
    });

    Ok(ImportOpts {
        dest: dest_slug,
        dir,
        files,
        start,
        end,
        pax: pax.unwrap_or(2),
        note,
        dry_run,
        plan_id,
    })
}

pub async fn run(opts: ImportOpts) -> Result<(), String> {
    // Collect file paths
    let file_paths: Vec<String> = if let Some(ref files_csv) = opts.files {
        files_csv
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        let dir = opts.dir.as_deref().unwrap_or("scrapes");
        let dir_path = Path::new(dir);
        if !dir_path.exists() || !dir_path.is_dir() {
            eprintln!("No JSON files found. Use --dir <path> or --files <csv>.");
            std::process::exit(1);
        }
        let mut paths: Vec<String> = Vec::new();
        let entries = std::fs::read_dir(dir_path).map_err(|e| format!("read_dir: {e}"))?;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".json") {
                continue;
            }
            // TS excludes: schema, destinations, ota-sources
            if name.contains("schema")
                || name.contains("destinations")
                || name.contains("ota-sources")
            {
                continue;
            }
            paths.push(dir_path.join(&name).to_string_lossy().to_string());
        }
        paths.sort();
        paths
    };

    if file_paths.is_empty() {
        eprintln!("No JSON files found. Use --dir <path> or --files <csv>.");
        std::process::exit(1);
    }

    // Connect to DB and resolve destination
    let conn = crate::db::connect_write().await?;
    let dest = if opts.dest.is_empty() {
        resolve_active_destination(&conn, &opts.plan_id).await?
    } else {
        opts.dest.clone()
    };

    // Print header
    println!(
        "Importing offers for {} from {} file(s){}...",
        dest,
        file_paths.len(),
        if opts.dry_run { " (dry-run)" } else { "" }
    );

    // Parse files
    let parse_opts = scrape_parser::ParseOpts {
        start: opts.start.clone(),
        end: opts.end.clone(),
        include_undated: true,
        pax: opts.pax,
    };
    let parsed = scrape_parser::parse_scrape_files(&file_paths, &parse_opts);

    // Build per-source counts
    let counts: Vec<(String, usize)> = parsed
        .iter()
        .filter(|g| !g.offers.is_empty())
        .map(|g| (g.source_id.clone(), g.offers.len()))
        .collect();

    if counts.is_empty() {
        println!("No offers imported (no matching files or all offers filtered out).");
        return Ok(());
    }

    // Print per-source counts
    for (src, count) in &counts {
        println!("  {src}: {count} offer(s)");
    }

    if opts.dry_run {
        return Ok(());
    }

    // Write to DB
    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();
    let version_before = read_version(&conn, &opts.plan_id).await?;
    let mut version_after = version_before;

    for group in &parsed {
        if group.offers.is_empty() {
            continue;
        }
        let source_id = &group.source_id;
        let note_str = match &opts.note {
            Some(n) => n.clone(),
            None => format!("import-offers: {}", group.file_name),
        };

        // Delete existing offers for these offer IDs (merge-by-id semantics)
        for offer in &group.offers {
            delete_offer_rows(&conn, &opts.plan_id, &dest, &offer.id).await?;
        }

        // Insert new offer rows
        for offer in &group.offers {
            insert_offer(&conn, &opts.plan_id, &dest, offer, &now_db).await?;
        }

        // Insert provenance
        insert_provenance(
            &conn,
            &opts.plan_id,
            &dest,
            source_id,
            &now_iso,
            &group.file_path,
            group.offers.len() as i64,
        )
        .await?;

        // Insert warnings
        for w in &group.warnings {
            insert_warning(&conn, &opts.plan_id, &dest, w).await?;
        }

        // Update process_statuses: P3_4 → researched if null/pending/researching
        let current_status = read_process_status(&conn, &opts.plan_id, &dest, P34).await?;
        let new_status: &str = match current_status.as_deref() {
            None | Some("pending") | Some("researching") => "researched",
            Some(s) => s,
        };
        upsert_process_status(
            &conn,
            &opts.plan_id,
            &dest,
            P34,
            new_status,
            &now_db,
        )
        .await?;

        // Emit events: package_offers_imported (dest_process + timeline)
        let kv: Vec<(&str, String)> = vec![
            ("source_id", source_id.clone()),
            ("offers_found", group.offers.len().to_string()),
            ("note", note_str.clone()),
        ];

        let dest_so = next_dest_process_sort_order(&conn, &opts.plan_id, &dest, P34).await?;
        insert_event(
            &conn,
            &opts.plan_id,
            "dest_process",
            &dest,
            P34,
            dest_so,
            "package_offers_imported",
            &now_iso,
            None,
            None,
        )
        .await?;
        insert_kv_rows(
            &conn,
            &opts.plan_id,
            "dest_process",
            &dest,
            P34,
            dest_so,
            &kv,
        )
        .await?;

        let tl_so = next_timeline_sort_order(&conn, &opts.plan_id).await?;
        insert_event(
            &conn,
            &opts.plan_id,
            "timeline",
            "",
            P34,
            tl_so,
            "package_offers_imported",
            &now_iso,
            None,
            None,
        )
        .await?;
        insert_kv_rows(
            &conn,
            &opts.plan_id,
            "timeline",
            "",
            P34,
            tl_so,
            &kv,
        )
        .await?;
    }

    version_after += 1;

    // operation_runs
    let run_id = new_run_id();
    let summary = format!(
        "{}: {}",
        dest,
        counts
            .iter()
            .map(|(k, v)| format!("{k}:{v}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    conn.execute(
        "INSERT INTO operation_runs \
            (run_id, plan_id, command_type, command_summary, status, \
             version_before, version_after, started_at, completed_at) \
         VALUES (?1, ?2, 'import-offers', ?3, 'completed', ?4, ?5, ?6, ?6)",
        libsql::params![run_id, opts.plan_id.clone(), summary, version_before, version_after, now_db.clone()],
    )
    .await
    .map_err(|e| e.to_string())?;

    // plans.version +1
    conn.execute(
        "UPDATE plans SET version = ?1, updated_at = ?2 WHERE plan_id = ?3",
        libsql::params![version_after, now_db, opts.plan_id.clone()],
    )
    .await
    .map_err(|e| e.to_string())?;

    println!("Saved to Turso (plan_offers).");
    Ok(())
}

// ============================================================================
// DB helpers
// ============================================================================

async fn resolve_active_destination(
    conn: &Connection,
    plan_id: &str,
) -> Result<String, String> {
    let mut rows = conn
        .query(
            "SELECT active_destination FROM plan_metadata WHERE plan_id = ?1",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_metadata query failed: {e}"))?;
    if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let dest: String = row.get(0).map_err(|e| e.to_string())?;
        if dest.is_empty() {
            return Err(format!(
                "plan_metadata.active_destination is empty for plan_id={plan_id}"
            ));
        }
        return Ok(dest);
    }
    Err(format!(
        "plan_metadata row missing for plan_id={plan_id} (no local-data fallback)"
    ))
}

/// Delete all rows for one offer_id across all offer tables (merge-by-id: DELETE then reinsert).
async fn delete_offer_rows(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    offer_id: &str,
) -> Result<(), String> {
    let tables = [
        "plan_offer_includes",
        "plan_offer_hotel_access",
        "plan_offer_date_pricing",
        "plan_offer_best_value",
        "plan_offer_flights",
        "plan_offer_hotels",
        "plan_offers",
    ];
    for table in &tables {
        conn.execute(
            &format!(
                "DELETE FROM {table} WHERE plan_id = ?1 AND destination = ?2 AND {} = ?3",
                if *table == "plan_offers" {
                    "id"
                } else {
                    "offer_id"
                }
            ),
            libsql::params![plan_id.to_string(), dest.to_string(), offer_id.to_string()],
        )
        .await
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Insert one offer and all its child rows.
async fn insert_offer(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    offer: &CanonicalOffer,
    now_db: &str,
) -> Result<(), String> {
    // plan_offers
    conn.execute(
        "INSERT INTO plan_offers \
            (plan_id, destination, id, source_id, type, title, price_per_person, currency, \
             availability, url, scraped_at, product_code, duration_days, price_total, \
             seats_remaining, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL, NULL, ?12, NULL, ?13)",
        libsql::params![
            plan_id.to_string(),
            dest.to_string(),
            offer.id.clone(),
            offer.source_id.clone(),
            offer.offer_type.clone(),
            offer.title.clone(),
            offer.price_per_person,
            offer.currency.clone(),
            offer.availability.clone(),
            offer.url.clone(),
            offer.scraped_at.clone(),
            offer.price_total,
            now_db.to_string(),
        ],
    )
    .await
    .map_err(|e| e.to_string())?;

    // includes → plan_offer_includes child rows
    for (ii, item) in offer.includes.iter().enumerate() {
        conn.execute(
            "INSERT INTO plan_offer_includes \
                (plan_id, destination, offer_id, sort_order, item) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            libsql::params![
                plan_id.to_string(),
                dest.to_string(),
                offer.id.clone(),
                ii as i64,
                item.clone(),
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
    }

    // flights
    if let Some(ref flight) = offer.flight {
        for (dir, leg) in [("outbound", &flight.outbound), ("return", &flight.return_leg)] {
            conn.execute(
                "INSERT INTO plan_offer_flights \
                    (plan_id, destination, offer_id, direction, flight_number, airline, \
                     airline_code, departure_code, departure_time, arrival_code, arrival_time, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                libsql::params![
                    plan_id.to_string(),
                    dest.to_string(),
                    offer.id.clone(),
                    dir.to_string(),
                    leg.flight_number.clone(),
                    // TS syncNormalizedTables reads `oflight.airline` / `oflight.airline_code`
                    // (flight-object level). CanonicalOffer carries airline per-leg with no
                    // flight-level field, so the representative value is the outbound leg's.
                    flight.outbound.airline.clone(),
                    flight.outbound.airline_code.clone(),
                    leg.departure_code.clone(),
                    leg.departure_time.clone(),
                    leg.arrival_code.clone(),
                    leg.arrival_time.clone(),
                    now_db.to_string(),
                ],
            )
            .await
            .map_err(|e| e.to_string())?;
        }
    }

    // hotel
    if let Some(ref hotel) = offer.hotel {
        conn.execute(
            "INSERT INTO plan_offer_hotels \
                (plan_id, destination, offer_id, name, slug, area, star_rating, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            libsql::params![
                plan_id.to_string(),
                dest.to_string(),
                offer.id.clone(),
                hotel.name.clone(),
                // TS flush writes sqlText(ohotel.slug) — NULL when slug is absent.
                // mapHotel never emits slug, so this is NULL for scraped offers.
                // Bind the Option directly (None => NULL), do NOT coerce to "".
                hotel.slug.clone(),
                hotel.area.clone(),
                hotel.star_rating,
                now_db.to_string(),
            ],
        )
        .await
        .map_err(|e| e.to_string())?;

        // access lines
        for (ai, line) in hotel.access.iter().enumerate() {
            conn.execute(
                "INSERT INTO plan_offer_hotel_access \
                    (plan_id, destination, offer_id, sort_order, line) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                libsql::params![
                    plan_id.to_string(),
                    dest.to_string(),
                    offer.id.clone(),
                    ai as i64,
                    line.clone(),
                ],
            )
            .await
            .map_err(|e| e.to_string())?;
        }
    }

    // date_pricing
    for dp in &offer.date_pricing {
        conn.execute(
            "INSERT INTO plan_offer_date_pricing \
                (plan_id, destination, offer_id, date, price, availability, seats_remaining, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            libsql::params![
                plan_id.to_string(),
                dest.to_string(),
                offer.id.clone(),
                dp.date.clone(),
                dp.price_per_person,
                dp.availability.clone(),
                dp.seats_remaining,
                now_db.to_string(),
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
    }

    // best_value
    if let Some(ref bv) = offer.best_value {
        conn.execute(
            "INSERT INTO plan_offer_best_value \
                (plan_id, destination, offer_id, best_date, best_price, currency, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            libsql::params![
                plan_id.to_string(),
                dest.to_string(),
                offer.id.clone(),
                bv.date.clone(),
                bv.price_per_person,
                offer.currency.clone(),
                now_db.to_string(),
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
    }

    Ok(())
}

async fn insert_provenance(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    source_id: &str,
    scraped_at: &str,
    file_path: &str,
    offer_count: i64,
) -> Result<(), String> {
    conn.execute(
        "INSERT OR IGNORE INTO plan_offer_provenance \
            (plan_id, destination, source_id, scraped_at, file_path, offer_count, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))",
        libsql::params![
            plan_id.to_string(),
            dest.to_string(),
            source_id.to_string(),
            scraped_at.to_string(),
            file_path.to_string(),
            offer_count,
        ],
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

async fn insert_warning(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    message: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO plan_offer_warnings \
            (plan_id, destination, warning_type, message) \
         VALUES (?1, ?2, 'parse', ?3)",
        libsql::params![
            plan_id.to_string(),
            dest.to_string(),
            message.to_string(),
        ],
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

async fn read_process_status(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    process_id: &str,
) -> Result<Option<String>, String> {
    let mut rows = conn
        .query(
            "SELECT status FROM process_statuses \
             WHERE plan_id = ?1 AND destination = ?2 AND process_id = ?3",
            libsql::params![
                plan_id.to_string(),
                dest.to_string(),
                process_id.to_string()
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let status: String = row.get(0).unwrap_or_default();
        if status.is_empty() {
            return Ok(None);
        }
        return Ok(Some(status));
    }
    Ok(None)
}

async fn upsert_process_status(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    process_id: &str,
    status: &str,
    now_db: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO process_statuses (plan_id, destination, process_id, status, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(plan_id, destination, process_id) DO UPDATE SET \
            status = excluded.status, updated_at = excluded.updated_at",
        libsql::params![
            plan_id.to_string(),
            dest.to_string(),
            process_id.to_string(),
            status.to_string(),
            now_db.to_string(),
        ],
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}
