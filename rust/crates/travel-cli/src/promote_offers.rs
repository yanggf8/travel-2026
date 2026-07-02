// `travel promote-offers --from-offers --dest <slug> [--plan-id <id>] [--source <id>]
//     [--start <date>] [--end <date>] [--pax N] [--dry-run]`
//
// Bridge: global `offers` table (the OTA capture landing zone) → the plan-scoped
// `plan_offers` family that `select-offer` consumes. The OTA sweep writes to `offers`;
// `select-offer` reads `plan_offers`/`plan_offer_date_pricing`/`plan_offer_hotels`/
// `plan_offer_flights`. These are disjoint table families — this command is the only
// bridge between them.
//
// Design + Codex review: docs/superpowers/specs/2026-06-28-promote-offers-bridge-design.md
//
// Structurally parallels import_offers.rs (merge-by-id delete + reinsert, P3_4→researched
// upsert, events, audit triad) but reads the `offers` TABLE instead of scrape JSON files.
//
// Differences from import_offers.rs (deliberate):
//   - reads `offers` (latest snapshot per id via MAX(scraped_at)) not files.
//   - audit triad back-half via cascade::common::record_operation (NOT hand-rolled —
//     import_offers.rs hand-rolls operation_runs + plans.version, which is the stale pattern).
//   - per-offer fail-soft skip when price_per_person/departure_date is NULL
//     (plan_offer_date_pricing.price is NOT NULL).
//   - flight legs written only when flight_outbound AND flight_return are non-NULL
//     (no fabrication from the offer-id string).

use crate::cascade::common::{
    insert_event, insert_kv_rows, next_dest_process_sort_order, next_timeline_sort_order,
    now_db_datetime, now_rfc3339, read_version, record_operation,
};
use libsql::Connection;
use travel_db::repo::plan_offers::{
    self, PlanOfferFlightWrite, PlanOfferHotelWrite, PlanOfferWrite,
};

const P34: &str = "process_3_4_packages";

pub struct PromoteOpts {
    pub dest: String,
    pub plan_id: String,
    pub source: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub pax: i64,
    pub dry_run: bool,
}

/// One offer read from the global `offers` table (latest snapshot per id).
struct SourceOffer {
    id: String,
    source_id: String,
    offer_type: String,
    name: Option<String>,
    price_per_person: Option<i64>,
    currency: Option<String>,
    availability: Option<String>,
    departure_date: Option<String>,
    hotel_name: Option<String>,
    hotel_area: Option<String>,
    flight_outbound: Option<String>,
    flight_return: Option<String>,
    includes: Option<String>,
    scraped_at: String,
}

/// Parse CLI args for promote-offers.
pub fn parse_args(rest: &[String]) -> Result<PromoteOpts, String> {
    if rest.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Usage:\n  travel promote-offers --from-offers --dest <slug> [--plan-id <id>] \
             [--source <id>] [--start <date>] [--end <date>] [--pax N] [--dry-run]"
        );
        std::process::exit(0);
    }

    let mut from_offers = false;
    let mut dest = None;
    let mut plan_id_arg = None;
    let mut source = None;
    let mut start = None;
    let mut end = None;
    let mut pax = None;
    let mut dry_run = false;

    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--from-offers" => from_offers = true,
            "--dest" => {
                i += 1;
                dest = rest.get(i).cloned();
            }
            "--plan-id" => {
                i += 1;
                plan_id_arg = rest.get(i).cloned();
            }
            "--source" => {
                i += 1;
                source = rest.get(i).cloned();
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
            "--dry-run" => dry_run = true,
            _ => {}
        }
        i += 1;
    }

    if !from_offers {
        return Err("Error: promote-offers requires --from-offers".to_string());
    }
    let dest = dest.ok_or_else(|| {
        "Error: promote-offers requires --dest <slug> (the global offers.destination)".to_string()
    })?;

    // plan_id: --plan-id wins, else $TRAVEL_PLAN_ID (matches import_offers parse_args).
    let plan_id = plan_id_arg
        .or_else(|| std::env::var("TRAVEL_PLAN_ID").ok())
        .unwrap_or_else(|| "test-set-dates-2026".to_string());

    Ok(PromoteOpts {
        dest,
        plan_id,
        source,
        start,
        end,
        pax: pax.unwrap_or(2),
        dry_run,
    })
}

pub async fn run(opts: PromoteOpts) -> Result<(), String> {
    let conn = crate::db::connect_write()
        .await
        .map_err(|e| format!("failed to connect to Turso (write tier): {e}"))?;

    let offers = read_source_offers(&conn, &opts).await?;

    println!(
        "Promoting offers for {} from global offers table{}...",
        opts.dest,
        if opts.dry_run { " (dry-run)" } else { "" }
    );

    if offers.is_empty() {
        println!("No offers to promote (no matching rows in offers table).");
        return Ok(());
    }

    // Group by source_id, splitting promotable vs skipped (NULL price/date).
    // Preserve first-seen source order (the read query orders by source_id, id).
    let mut order: Vec<String> = Vec::new();
    let mut promoted: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut skipped: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut to_write: Vec<&SourceOffer> = Vec::new();

    for o in &offers {
        if !order.contains(&o.source_id) {
            order.push(o.source_id.clone());
        }
        let has_price = o.price_per_person.is_some();
        let has_date = o.departure_date.as_deref().map(|s| !s.is_empty()).unwrap_or(false);
        if has_price && has_date {
            *promoted.entry(o.source_id.clone()).or_insert(0) += 1;
            to_write.push(o);
        } else {
            *skipped.entry(o.source_id.clone()).or_insert(0) += 1;
        }
    }

    // Per-source counts (printed for both dry-run and real run).
    for src in &order {
        let p = promoted.get(src).copied().unwrap_or(0);
        let s = skipped.get(src).copied().unwrap_or(0);
        let skip_note = if s > 0 {
            format!(" — {s} skipped (no price/date)")
        } else {
            String::new()
        };
        println!("  {src}: {p} offer(s){skip_note}");
    }

    if opts.dry_run {
        return Ok(());
    }

    if to_write.is_empty() {
        // All matching rows were skipped — nothing written, no version bump.
        println!("Saved to Turso (plan_offers).");
        return Ok(());
    }

    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();
    let version_before = read_version(&conn, &opts.plan_id).await?;
    let version_after = version_before + 1;

    // --- Write each promotable offer (merge-by-id: delete then reinsert). ---
    for o in &to_write {
        plan_offers::delete_offer_rows(&conn, &opts.plan_id, &opts.dest, &o.id).await?;
        let write = build_plan_offer_write(&opts.plan_id, &opts.dest, o, opts.pax);
        plan_offers::insert_offer(&conn, &write, &now_db).await?;
    }

    // --- P3_4 → researched if null/pending/researching (UPSERT). ---
    let current_status = read_process_status(&conn, &opts.plan_id, &opts.dest, P34).await?;
    let new_status: &str = match current_status.as_deref() {
        None | Some("pending") | Some("researching") => "researched",
        Some(s) => s,
    };
    travel_db::repo::process_statuses::upsert(&conn, &opts.plan_id, &opts.dest, P34, new_status, &now_db)
        .await?;

    // --- Events: package_offers_promoted (dest_process + timeline), per source. ---
    for src in &order {
        let count = promoted.get(src).copied().unwrap_or(0);
        if count == 0 {
            continue;
        }
        let kv: Vec<(&str, String)> = vec![
            ("source_id", src.clone()),
            ("offers_found", count.to_string()),
            ("note", format!("promote-offers: {src}")),
        ];

        let dest_so =
            next_dest_process_sort_order(&conn, &opts.plan_id, &opts.dest, P34).await?;
        insert_event(
            &conn,
            &opts.plan_id,
            "dest_process",
            &opts.dest,
            P34,
            dest_so,
            "package_offers_promoted",
            &now_iso,
            None,
            None,
        )
        .await?;
        insert_kv_rows(&conn, &opts.plan_id, "dest_process", &opts.dest, P34, dest_so, &kv)
            .await?;

        let tl_so = next_timeline_sort_order(&conn, &opts.plan_id).await?;
        insert_event(
            &conn,
            &opts.plan_id,
            "timeline",
            "",
            P34,
            tl_so,
            "package_offers_promoted",
            &now_iso,
            None,
            None,
        )
        .await?;
        insert_kv_rows(&conn, &opts.plan_id, "timeline", "", P34, tl_so, &kv).await?;
    }

    // --- Audit triad back-half: operation_runs + plans.version (shared helper). ---
    let summary = format!(
        "{}: {}",
        opts.dest,
        order
            .iter()
            .filter_map(|s| {
                let c = promoted.get(s).copied().unwrap_or(0);
                if c > 0 {
                    Some(format!("{s}:{c}"))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    );
    record_operation(
        &conn,
        &opts.plan_id,
        "promote-offers",
        &summary,
        version_before,
        version_after,
        &now_db,
    )
    .await?;

    println!("Saved to Turso (plan_offers).");
    Ok(())
}

// ============================================================================
// DB helpers
// ============================================================================

/// Read the latest snapshot per id from the global `offers` table for this
/// destination, with optional source/date filters. PK is (id, scraped_at), so a
/// MAX(scraped_at) self-join takes the freshest snapshot per id.
async fn read_source_offers(
    conn: &Connection,
    opts: &PromoteOpts,
) -> Result<Vec<SourceOffer>, String> {
    // Build the WHERE incrementally with positional params.
    // ?1 = destination (used twice). Following params appended as needed.
    let mut sql = String::from(
        "SELECT o.id, o.source_id, o.type, o.name, o.price_per_person, o.currency, \
                o.availability, o.departure_date, o.hotel_name, o.hotel_area, \
                o.flight_outbound, o.flight_return, o.includes, o.scraped_at \
         FROM offers o \
         JOIN (SELECT id, MAX(scraped_at) AS latest FROM offers \
               WHERE destination = ?1 GROUP BY id) m \
           ON o.id = m.id AND o.scraped_at = m.latest \
         WHERE o.destination = ?1",
    );
    let mut params: Vec<libsql::Value> = vec![opts.dest.clone().into()];
    let mut next = 2;
    if let Some(ref src) = opts.source {
        sql.push_str(&format!(" AND o.source_id = ?{next}"));
        params.push(src.clone().into());
        next += 1;
    }
    if let Some(ref start) = opts.start {
        sql.push_str(&format!(" AND o.departure_date >= ?{next}"));
        params.push(start.clone().into());
        next += 1;
    }
    if let Some(ref end) = opts.end {
        sql.push_str(&format!(" AND o.departure_date <= ?{next}"));
        params.push(end.clone().into());
    }
    sql.push_str(" ORDER BY o.source_id, o.id");

    let mut rows = conn
        .query(&sql, params)
        .await
        .map_err(|e| format!("offers query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        out.push(SourceOffer {
            id: row.get(0).map_err(|e| e.to_string())?,
            source_id: row.get(1).map_err(|e| e.to_string())?,
            offer_type: row.get(2).map_err(|e| e.to_string())?,
            name: opt(row.get::<String>(3)),
            price_per_person: row.get::<i64>(4).ok(),
            currency: opt(row.get::<String>(5)),
            availability: opt(row.get::<String>(6)),
            departure_date: opt(row.get::<String>(7)),
            hotel_name: opt(row.get::<String>(8)),
            hotel_area: opt(row.get::<String>(9)),
            flight_outbound: opt(row.get::<String>(10)),
            flight_return: opt(row.get::<String>(11)),
            includes: opt(row.get::<String>(12)),
            scraped_at: row.get(13).map_err(|e| e.to_string())?,
        });
    }
    Ok(out)
}

/// Map a promotable `SourceOffer` to the DAL write payload. Business decisions
/// (title fallback, hotel/flight presence, includes split) stay in the command.
fn build_plan_offer_write(
    plan_id: &str,
    dest: &str,
    o: &SourceOffer,
    pax: i64,
) -> PlanOfferWrite {
    let price = o.price_per_person.expect("filtered: price non-NULL");
    let departure_date = o.departure_date.clone().expect("filtered: date non-NULL");
    let title = match o.name.as_deref() {
        Some(n) if !n.is_empty() => Some(n.to_string()),
        _ => o.hotel_name.clone(),
    };
    let price_total = price * pax;

    let hotel = o.hotel_name.as_ref().and_then(|hotel_name| {
        if hotel_name.is_empty() {
            None
        } else {
            let area = match o.hotel_area.as_deref() {
                Some(a) if !a.is_empty() => Some(a.to_string()),
                _ => None,
            };
            Some(PlanOfferHotelWrite {
                name: hotel_name.clone(),
                area,
            })
        }
    });

    let flights = match (&o.flight_outbound, &o.flight_return) {
        (Some(outbound), Some(ret)) => Some([
            PlanOfferFlightWrite {
                direction: "outbound".to_string(),
                flight_number: outbound.clone(),
            },
            PlanOfferFlightWrite {
                direction: "return".to_string(),
                flight_number: ret.clone(),
            },
        ]),
        _ => None,
    };

    let includes = o
        .includes
        .as_ref()
        .map(|inc| {
            inc.split(['\n', ';', '、', ','])
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    PlanOfferWrite {
        plan_id: plan_id.to_string(),
        destination: dest.to_string(),
        offer_id: o.id.clone(),
        source_id: o.source_id.clone(),
        offer_type: o.offer_type.clone(),
        title,
        price_per_person: price,
        currency: o.currency.clone(),
        availability: o.availability.clone(),
        scraped_at: o.scraped_at.clone(),
        price_total,
        departure_date,
        hotel,
        flights,
        includes,
    }
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
            libsql::params![plan_id.to_string(), dest.to_string(), process_id.to_string()],
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

/// Map a libsql column read to Option<String>, treating NULL/error and the empty
/// string as None (matches select_offer.rs::opt).
fn opt(r: Result<String, libsql::Error>) -> Option<String> {
    match r {
        Ok(s) if !s.is_empty() => Some(s),
        _ => None,
    }
}
