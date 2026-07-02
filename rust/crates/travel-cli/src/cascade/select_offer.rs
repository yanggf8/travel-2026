// `travel select-offer <offer-id> <date> [--no-populate]` cascade write.
//
// Port of the TS StateManager.selectOffer() + populateFromOffer() +
// saveWithTracking('select-offer', offerId) path. This is the
// "populate P3+P4 from the chosen package" cascade keystone.
//
// The acceptance gate is row parity against
//   TRAVEL_PLAN_ID=test-set-dates-2026 \
//     npm run travel -- select-offer <offer> <date>
// for the disposable plan (modulo the volatile fields listed below),
// measured by before/after DB-row diff.
//
// Writes (active destination only):
//   1. plan_offer_selection  — INSERT selected_offer_id, selected_date,
//      selected_at. QUIRK: selected_date is NULL, not <date>. The TS
//      writer reads it from `results.selection.date` which selectOffer
//      never populates (it writes `chosen_offer` instead) → NULL.
//      selected_at = the run timestamp (ISO).
//   2. process_statuses      — P3_4 researched→selected; if the offer has
//      a flight P3 →populated; if it has a hotel P4 →populated.
//   3. flight_legs           — (populate, if flight) DELETE all legs for
//      the destination, reinsert one row per offer flight direction.
//      airline/airline_code come from the offer's flights; per-leg
//      flight_number/dep+arr code/time. departure_airport/terminal,
//      arrival_airport/terminal and flight_date are NULL (the assembled
//      offer.flight does not carry them). booked_date = <date>.
//      populated_from = `package:<offer_id>`.
//   4. hotels                — (populate, if hotel) DELETE the dest's hotel
//      row, reinsert name/check_in(=<date>)/populated_from.
//   5. hotel_access_lines    — (populate, if hotel) DELETE then reinsert
//      one row per offer hotel-access line.
//   6. plan_events / plan_event_data — see EVENT ORDER below.
//   7. operation_runs        — 1 row, command_type='select-offer',
//      command_summary=<offer_id>.
//   8. plans.version         — +1.
//
// Does NOT write (verified absent from the TS delta):
//   - cascade_dirty_flags (clearDirty P3/P4 is a no-op when already clean)
//
// EVENT ORDER (emission order = TS selectOffer/populateFromOffer):
//   1. status_changed   P3_4 researched→selected   (dest_process + timeline)
//   2. offer_selected   P3_4                        (dest_process + timeline)
//        KV: date, hotel, offer_id, offer_name=undefined, price_total
//   3. status_changed   P3 pending→populated        (dest_process + timeline)
//        [only if offer has a flight]
//   4. status_changed   P4 pending→populated        (dest_process + timeline)
//        [only if offer has a hotel]
//   5. cascade_populated                            (timeline ONLY)
//        KV: populated="process_3_transportation, process_4_accommodation",
//            source="package:<offer_id>"
//
// dest_process sort_order is per-(destination, process_id) bucket;
// timeline sort_order is a single global index — both = MAX(existing)+1,
// incremented per appended event in emission order.
//
// Volatile fields (normalized out of the diff):
//   - updated_at / last_changed on every row
//   - operation_runs.run_id / started_at / completed_at
//   - plan_events.event_at; plan_offer_selection.selected_at

use super::common::{
    emit_event_both, emit_status_changed, insert_event, insert_kv_rows,
    next_timeline_sort_order, now_db_datetime, now_rfc3339, read_version,
    validate_transition,
};
use crate::status::format_date;
use libsql::Connection;

const P34: &str = "process_3_4_packages";
const P3: &str = "process_3_transportation";
const P4: &str = "process_4_accommodation";

/// One flight leg from the chosen offer (`plan_offer_flights`).
struct OfferFlightLeg {
    direction: String, // "outbound" | "return"
    flight_number: Option<String>,
    departure_code: Option<String>,
    departure_time: Option<String>,
    arrival_code: Option<String>,
    arrival_time: Option<String>,
}

/// The chosen offer's denormalized flight/hotel/price data, read once.
struct OfferData {
    airline: Option<String>,
    airline_code: Option<String>,
    legs: Vec<OfferFlightLeg>,
    hotel_name: Option<String>,
    hotel_access: Vec<String>,
    price_total: Option<String>, // date_pricing[date].price as text, for the event KV
}

impl OfferData {
    fn has_flight(&self) -> bool {
        !self.legs.is_empty()
    }
    fn has_hotel(&self) -> bool {
        self.hotel_name.is_some()
    }
}

/// Run select-offer. `plan_id` is the resolved plan; `populate` mirrors
/// `!--no-populate` (default true). Prints the TS header lines, then
/// performs the cascade write.
pub async fn run(
    offer_id: String,
    date: String,
    populate: bool,
    plan_id: String,
) -> Result<(), String> {
    // Header (byte-parity with offers.ts selectOfferCommand).
    println!("\n🎯 Selecting offer:");
    println!("   Offer: {offer_id}");
    println!("   Date: {}", format_date(&date));
    println!("   Populate P3/P4: {populate}");

    let conn = crate::db::connect_write()
        .await
        .map_err(|e| format!("failed to connect to Turso (write tier): {e}"))?;

    let dest = read_active_destination(&conn, &plan_id).await?;
    let offer = read_offer_data(&conn, &plan_id, &dest, &offer_id, &date).await?;

    execute(&conn, &plan_id, &dest, &offer_id, &date, populate, &offer).await?;

    println!("✅ Offer selected");
    if populate {
        // Report only what the offer actually populated. A package with no flight legs
        // populates P4 (accommodation) but not P3 (transportation) — claiming both is
        // misleading (GitHub issue #5 item 3).
        match (offer.has_flight(), offer.has_hotel()) {
            (true, true) => println!(
                "✅ P3 (transportation) and P4 (accommodation) populated from package"
            ),
            (true, false) => println!("✅ P3 (transportation) populated from package"),
            (false, true) => println!("✅ P4 (accommodation) populated from package"),
            (false, false) => {
                println!("ℹ️  Offer has no flight or hotel — nothing to populate")
            }
        }
    }
    Ok(())
}

/// The full cascade write. Kept separate from `run` so the side-effect
/// surface is one auditable function (matches date_change::execute).
#[allow(clippy::too_many_arguments)]
async fn execute(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    offer_id: &str,
    date: &str,
    populate: bool,
    offer: &OfferData,
) -> Result<i64, String> {
    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();
    let version_before = read_version(conn, plan_id).await?;
    let version_after = version_before + 1;

    // --- 0. Validate the P3_4 transition BEFORE any write. The TS path
    //    mutates the offer selection in memory first, but throws inside
    //    setProcessStatus before save() ever flushes — so on an invalid
    //    transition NOTHING is persisted. Validating up front gives the
    //    same all-or-nothing behavior (no stray plan_offer_selection row).
    let p34_from = read_status(conn, plan_id, dest, P34).await?;
    validate_transition(p34_from.as_deref(), "selected", dest, P34)?;

    // --- 1. plan_offer_selection (DELETE-then-insert). selected_date=NULL. ---
    travel_db::repo::plan_offers::set_selection(conn, plan_id, dest, offer_id, &now_iso, &now_db)
        .await?;

    // --- 2. P3_4 researched→selected (UPDATE + status_changed) ---
    set_status(conn, plan_id, dest, P34, "selected", &now_db).await?;
    emit_status_changed(conn, plan_id, dest, P34, p34_from.as_deref(), "selected", &now_iso)
        .await?;

    // --- 3. offer_selected event (P3_4) ---
    {
        // offer_name is `String(offer.name)` where the assembled offer has
        // no `name` field → "undefined". Faithful to the TS KV value.
        let kv: Vec<(&str, String)> = vec![
            ("date", date.to_string()),
            ("hotel", offer.hotel_name.clone().unwrap_or_else(|| "undefined".to_string())),
            ("offer_id", offer_id.to_string()),
            ("offer_name", "undefined".to_string()),
            ("price_total", offer.price_total.clone().unwrap_or_else(|| "undefined".to_string())),
        ];
        emit_event_both(conn, plan_id, dest, P34, "offer_selected", &now_iso, None, None, &kv)
            .await?;
    }

    // --- 4. populate cascade (if requested) ---
    if populate {
        if offer.has_flight() {
            rewrite_flight_legs(conn, plan_id, dest, offer_id, date, offer, &now_db).await?;
            let p3_from = read_status(conn, plan_id, dest, P3).await?;
            validate_transition(p3_from.as_deref(), "populated", dest, P3)?;
            set_status(conn, plan_id, dest, P3, "populated", &now_db).await?;
            emit_status_changed(conn, plan_id, dest, P3, p3_from.as_deref(), "populated", &now_iso)
                .await?;
        }
        if offer.has_hotel() {
            rewrite_hotel(conn, plan_id, dest, offer_id, date, offer, &now_db).await?;
            let p4_from = read_status(conn, plan_id, dest, P4).await?;
            validate_transition(p4_from.as_deref(), "populated", dest, P4)?;
            set_status(conn, plan_id, dest, P4, "populated", &now_db).await?;
            emit_status_changed(conn, plan_id, dest, P4, p4_from.as_deref(), "populated", &now_iso)
                .await?;
        }

        // cascade_populated — TIMELINE ONLY (no dest_process row).
        let so = next_timeline_sort_order(conn, plan_id).await?;
        insert_event(
            conn,
            plan_id,
            "timeline",
            "",
            "",
            so,
            "cascade_populated",
            &now_iso,
            None,
            None,
        )
        .await?;
        insert_kv_rows(
            conn,
            plan_id,
            "timeline",
            "",
            "",
            so,
            &[
                (
                    "populated",
                    "process_3_transportation, process_4_accommodation".to_string(),
                ),
                ("source", format!("package:{offer_id}")),
            ],
        )
        .await?;
    }

    // --- 5. operation_runs + plans.version (shared audit-triad back half) ---
    super::common::record_operation(
        conn,
        plan_id,
        "select-offer",
        offer_id,
        version_before,
        version_after,
        &now_db,
    )
    .await?;

    Ok(version_after)
}

/// DELETE all flight_legs for the destination, reinsert one row per offer
/// flight leg. Mirrors syncNormalizedTables: the assembled offer.flight
/// carries airline/airline_code (parent) + per-direction flight_number,
/// dep/arr code + time. Airport names, terminals and flight_date are NULL.
async fn rewrite_flight_legs(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    offer_id: &str,
    date: &str,
    offer: &OfferData,
    now_db: &str,
) -> Result<(), String> {
    use travel_db::repo::flight_legs::OfferFlightLegWrite;
    let legs: Vec<OfferFlightLegWrite> = offer
        .legs
        .iter()
        .map(|leg| OfferFlightLegWrite {
            direction: leg.direction.clone(),
            flight_number: leg.flight_number.clone(),
            departure_code: leg.departure_code.clone(),
            departure_time: leg.departure_time.clone(),
            arrival_code: leg.arrival_code.clone(),
            arrival_time: leg.arrival_time.clone(),
        })
        .collect();
    let populated_from = format!("package:{offer_id}");
    travel_db::repo::flight_legs::replace_from_offer(
        conn,
        plan_id,
        dest,
        &legs,
        offer.airline.as_deref(),
        offer.airline_code.as_deref(),
        &populated_from,
        date,
        now_db,
    )
    .await
}

/// DELETE the destination's hotel row + access lines, reinsert from offer.
async fn rewrite_hotel(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    offer_id: &str,
    date: &str,
    offer: &OfferData,
    now_db: &str,
) -> Result<(), String> {
    let input = travel_db::repo::hotels::OfferHotelWrite {
        name: offer.hotel_name.clone(),
        check_in: date.to_string(),
        populated_from: format!("package:{offer_id}"),
        access: offer.hotel_access.clone(),
    };
    travel_db::repo::hotels::replace_from_offer(conn, plan_id, dest, &input, now_db).await
}

// ---------------------------------------------------------------------------
// Status helpers
// ---------------------------------------------------------------------------

/// Read a process status, or None if the row is missing.
async fn read_status(
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
        let s: String = row.get(0).unwrap_or_default();
        return Ok(if s.is_empty() { None } else { Some(s) });
    }
    Ok(None)
}

async fn set_status(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    process_id: &str,
    status: &str,
    now_db: &str,
) -> Result<(), String> {
    // UPSERT, not a bare UPDATE: a real plan carries the full P1–P5 status ladder, but a
    // plan whose ladder lacks this process_id (e.g. seeded without it) would otherwise have
    // this write silently no-op, leaving the populate cascade invisible (GitHub issue #5).
    // INSERT-on-missing keeps the populate honest; ON CONFLICT preserves the UPDATE path.
    travel_db::repo::process_statuses::upsert(conn, plan_id, dest, process_id, status, now_db).await
}

// ---------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------

async fn read_active_destination(conn: &Connection, plan_id: &str) -> Result<String, String> {
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

/// Read the chosen offer's flight/hotel/access + per-date price. THROWS if
/// the offer row is missing (no local-data fallback). flights/hotel may be
/// empty (offer with no flight or no hotel — populate skips that process).
async fn read_offer_data(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    offer_id: &str,
    date: &str,
) -> Result<OfferData, String> {
    // Confirm the offer exists.
    let mut orows = conn
        .query(
            "SELECT id FROM plan_offers WHERE plan_id = ?1 AND destination = ?2 AND id = ?3",
            libsql::params![plan_id.to_string(), dest.to_string(), offer_id.to_string()],
        )
        .await
        .map_err(|e| e.to_string())?;
    if orows.next().await.map_err(|e| e.to_string())?.is_none() {
        return Err(format!(
            "offer not found: {offer_id} (plan_id={plan_id}, dest={dest}) — no local-data fallback"
        ));
    }

    // Flights (ordered so outbound precedes return deterministically).
    let mut frows = conn
        .query(
            "SELECT direction, flight_number, airline, airline_code, \
                    departure_code, departure_time, arrival_code, arrival_time \
             FROM plan_offer_flights \
             WHERE plan_id = ?1 AND destination = ?2 AND offer_id = ?3 \
             ORDER BY CASE direction WHEN 'outbound' THEN 0 ELSE 1 END",
            libsql::params![plan_id.to_string(), dest.to_string(), offer_id.to_string()],
        )
        .await
        .map_err(|e| e.to_string())?;
    let mut legs = Vec::new();
    let mut airline = None;
    let mut airline_code = None;
    while let Some(row) = frows.next().await.map_err(|e| e.to_string())? {
        let direction: String = row.get(0).unwrap_or_default();
        let flight_number: Option<String> = opt(row.get::<String>(1));
        let air: Option<String> = opt(row.get::<String>(2));
        let air_code: Option<String> = opt(row.get::<String>(3));
        // The assembled offer.flight takes airline/airline_code from the
        // FIRST flight row (plan-assembler.ts:234 flights[0]).
        if airline.is_none() {
            airline = air;
        }
        if airline_code.is_none() {
            airline_code = air_code;
        }
        legs.push(OfferFlightLeg {
            direction,
            flight_number,
            departure_code: opt(row.get::<String>(4)),
            departure_time: opt(row.get::<String>(5)),
            arrival_code: opt(row.get::<String>(6)),
            arrival_time: opt(row.get::<String>(7)),
        });
    }

    // Hotel (0 or 1 row).
    let mut hrows = conn
        .query(
            "SELECT name FROM plan_offer_hotels \
             WHERE plan_id = ?1 AND destination = ?2 AND offer_id = ?3",
            libsql::params![plan_id.to_string(), dest.to_string(), offer_id.to_string()],
        )
        .await
        .map_err(|e| e.to_string())?;
    let hotel_name = if let Some(row) = hrows.next().await.map_err(|e| e.to_string())? {
        opt(row.get::<String>(0))
    } else {
        None
    };

    // Hotel access lines (ordered by sort_order).
    let mut arows = conn
        .query(
            "SELECT line FROM plan_offer_hotel_access \
             WHERE plan_id = ?1 AND destination = ?2 AND offer_id = ?3 \
             ORDER BY sort_order",
            libsql::params![plan_id.to_string(), dest.to_string(), offer_id.to_string()],
        )
        .await
        .map_err(|e| e.to_string())?;
    let mut hotel_access = Vec::new();
    while let Some(row) = arows.next().await.map_err(|e| e.to_string())? {
        if let Some(line) = opt(row.get::<String>(0)) {
            hotel_access.push(line);
        }
    }

    // Per-date price (date_pricing[date].price → the offer_selected KV).
    let mut prows = conn
        .query(
            "SELECT price FROM plan_offer_date_pricing \
             WHERE plan_id = ?1 AND destination = ?2 AND offer_id = ?3 AND date = ?4",
            libsql::params![
                plan_id.to_string(),
                dest.to_string(),
                offer_id.to_string(),
                date.to_string()
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
    let price_total = if let Some(row) = prows.next().await.map_err(|e| e.to_string())? {
        // price is stored as INTEGER; render it the way String(number) would.
        match row.get::<i64>(0) {
            Ok(n) => Some(n.to_string()),
            Err(_) => opt(row.get::<String>(0)),
        }
    } else {
        None
    };

    Ok(OfferData {
        airline,
        airline_code,
        legs,
        hotel_name,
        hotel_access,
        price_total,
    })
}

/// Map a libsql column read to Option<String>, treating NULL/error and the
/// empty string as None.
fn opt(r: Result<String, libsql::Error>) -> Option<String> {
    match r {
        Ok(s) if !s.is_empty() => Some(s),
        _ => None,
    }
}
