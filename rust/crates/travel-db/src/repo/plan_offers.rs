//! `plan_offers` family domain writes for `promote-offers`, `import-offers`
//! (and future `select_offer` DAL migrations).
//!
//! DAL boundary: owns the domain-table SQL (the `plan_offer_*` DELETE/INSERT +
//! `process_statuses` upsert). The audit triad (`plan_events`/`plan_event_data`/
//! `operation_runs`/`plans.version`) stays in `travel-cli` (`cascade::common`) —
//! this module never touches it.

use libsql::Connection;

/// Typed payload for one promoted offer + its child rows.
#[derive(Debug, Clone)]
pub struct PlanOfferWrite {
    pub plan_id: String,
    pub destination: String,
    pub offer_id: String,
    pub source_id: String,
    pub offer_type: String,
    pub title: Option<String>,
    pub price_per_person: i64,
    pub currency: Option<String>,
    pub availability: Option<String>,
    pub scraped_at: String,
    pub price_total: i64,
    pub departure_date: String,
    pub hotel: Option<PlanOfferHotelWrite>,
    /// Flight legs to write (0, 1, or 2). Empty = no legs; one element is typically
    /// outbound-only (e.g. google_flights round-trip total with no paired return).
    pub flights: Vec<PlanOfferFlightWrite>,
    pub includes: Vec<String>,
}

/// One hotel child row (written only when `hotel` is `Some`).
#[derive(Debug, Clone)]
pub struct PlanOfferHotelWrite {
    pub name: String,
    pub area: Option<String>,
}

/// One flight leg child row (one entry per direction the caller supplies).
#[derive(Debug, Clone)]
pub struct PlanOfferFlightWrite {
    pub direction: String,
    pub flight_number: String,
}

/// Delete all rows for one offer_id across the plan_offer_* family (merge-by-id:
/// DELETE then reinsert).
pub async fn delete_offer_rows(
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
                if *table == "plan_offers" { "id" } else { "offer_id" }
            ),
            libsql::params![plan_id.to_string(), dest.to_string(), offer_id.to_string()],
        )
        .await
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Insert one promotable offer + its child rows. Caller has already filtered out NULL
/// price/date and shaped the optional hotel/flights/includes fields.
pub async fn insert_offer(
    conn: &Connection,
    write: &PlanOfferWrite,
    now_db: &str,
) -> Result<(), String> {
    // plan_offers
    conn.execute(
        "INSERT INTO plan_offers \
            (plan_id, destination, id, source_id, type, title, price_per_person, currency, \
             availability, url, scraped_at, product_code, duration_days, price_total, \
             seats_remaining, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, ?10, NULL, NULL, ?11, NULL, ?12)",
        libsql::params![
            write.plan_id.clone(),
            write.destination.clone(),
            write.offer_id.clone(),
            write.source_id.clone(),
            write.offer_type.clone(),
            write.title.clone(),
            write.price_per_person,
            write.currency.clone(),
            write.availability.clone(),
            write.scraped_at.clone(),
            write.price_total,
            now_db.to_string(),
        ],
    )
    .await
    .map_err(|e| e.to_string())?;

    // plan_offer_date_pricing — one synthesized row (price is NOT NULL).
    conn.execute(
        "INSERT INTO plan_offer_date_pricing \
            (plan_id, destination, offer_id, date, price, availability, seats_remaining, \
             currency, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?8)",
        libsql::params![
            write.plan_id.clone(),
            write.destination.clone(),
            write.offer_id.clone(),
            write.departure_date.clone(),
            write.price_per_person,
            write.availability.clone(),
            write.currency.clone(),
            now_db.to_string(),
        ],
    )
    .await
    .map_err(|e| e.to_string())?;

    // plan_offer_hotels — only if hotel present.
    if let Some(ref hotel) = write.hotel {
        conn.execute(
            "INSERT INTO plan_offer_hotels \
                (plan_id, destination, offer_id, name, slug, area, star_rating, updated_at) \
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, NULL, ?6)",
            libsql::params![
                write.plan_id.clone(),
                write.destination.clone(),
                write.offer_id.clone(),
                hotel.name.clone(),
                hotel.area.clone(),
                now_db.to_string(),
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
    }

    // plan_offer_flights — one row per leg the caller supplied (0, 1, or 2).
    for leg in &write.flights {
        conn.execute(
            "INSERT INTO plan_offer_flights \
                (plan_id, destination, offer_id, direction, flight_number, airline, \
                 airline_code, departure_code, departure_time, arrival_code, arrival_time, \
                 updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, NULL, NULL, NULL, NULL, ?6)",
            libsql::params![
                write.plan_id.clone(),
                write.destination.clone(),
                write.offer_id.clone(),
                leg.direction.clone(),
                leg.flight_number.clone(),
                now_db.to_string(),
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
    }

    // plan_offer_includes — caller already split/trimmed/filtered.
    for (ii, item) in write.includes.iter().enumerate() {
        conn.execute(
            "INSERT INTO plan_offer_includes \
                (plan_id, destination, offer_id, sort_order, item) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            libsql::params![
                write.plan_id.clone(),
                write.destination.clone(),
                write.offer_id.clone(),
                ii as i64,
                item.clone(),
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// Point-UPSERT one `plan_offer_date_pricing` (offer, date) row — used by `update-offer` to change a
/// single date's price/availability/seats. Distinct from `insert_offer`'s bulk INSERT: this ON CONFLICT
/// updates only price/availability/seats_remaining/updated_at and never touches `currency`. Caller has
/// already merged omitted values against the existing row.
#[allow(clippy::too_many_arguments)]
pub async fn upsert_date_pricing(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    offer_id: &str,
    date: &str,
    price: i64,
    availability: &str,
    seats_remaining: Option<i64>,
    now_db: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO plan_offer_date_pricing \
            (plan_id, destination, offer_id, date, price, availability, seats_remaining, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT(plan_id, destination, offer_id, date) DO UPDATE SET \
            price = excluded.price, availability = excluded.availability, \
            seats_remaining = excluded.seats_remaining, updated_at = excluded.updated_at",
        libsql::params![
            plan_id.to_string(),
            dest.to_string(),
            offer_id.to_string(),
            date.to_string(),
            price,
            availability.to_string(),
            seats_remaining,
            now_db.to_string(),
        ],
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Set the destination's chosen-offer selection — the `select-offer` keystone. DELETE-then-INSERT,
/// byte-identical to the inline SQL in `cascade::select_offer`: `selected_date` is written NULL
/// (the documented quirk — the CLI date arg is NOT stored here), `selected_at` = the run ISO
/// timestamp.
pub async fn set_selection(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    offer_id: &str,
    selected_at_iso: &str,
    now_db: &str,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM plan_offer_selection WHERE plan_id = ?1 AND destination = ?2",
        libsql::params![plan_id.to_string(), dest.to_string()],
    )
    .await
    .map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO plan_offer_selection \
            (plan_id, destination, selected_offer_id, selected_date, selected_at, updated_at) \
         VALUES (?1, ?2, ?3, NULL, ?4, ?5)",
        libsql::params![
            plan_id.to_string(),
            dest.to_string(),
            offer_id.to_string(),
            selected_at_iso.to_string(),
            now_db.to_string()
        ],
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Payload for one imported offer + all its child rows (import-offers shape:
/// url present, multi-row date_pricing without currency, full hotel + access,
/// full flights, best_value). Distinct from `PlanOfferWrite` (promote's shape).
#[derive(Debug, Clone)]
pub struct ImportPlanOfferWrite {
    pub plan_id: String,
    pub destination: String,
    pub offer_id: String,
    pub source_id: String,
    pub offer_type: String,
    pub title: String,
    pub price_per_person: i64,
    pub currency: String,
    pub availability: String,
    pub url: String,
    pub scraped_at: String,
    pub price_total: Option<i64>,
    pub includes: Vec<String>,
    pub flights: Option<ImportFlightPair>,
    pub hotel: Option<ImportHotelWrite>,
    pub date_pricing: Vec<ImportDatePricing>,
    pub best_value: Option<ImportBestValue>,
}

#[derive(Debug, Clone)]
pub struct ImportFlightPair {
    pub outbound: ImportFlightLeg,
    pub return_leg: ImportFlightLeg,
    pub airline: String,
    pub airline_code: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ImportFlightLeg {
    pub direction: String,
    pub flight_number: String,
    pub departure_code: String,
    pub departure_time: String,
    pub arrival_code: String,
    pub arrival_time: String,
}

#[derive(Debug, Clone)]
pub struct ImportHotelWrite {
    pub name: String,
    pub slug: Option<String>,
    pub area: String,
    pub star_rating: Option<i64>,
    pub access: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ImportDatePricing {
    pub date: String,
    pub price: i64,
    pub availability: String,
    pub seats_remaining: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ImportBestValue {
    pub date: String,
    pub price: i64,
    pub currency: String,
}

/// Insert one imported offer + all child rows. BYTE-IDENTICAL to the SQL that was inline in
/// import_offers.rs::insert_offer.
pub async fn insert_import_offer(
    conn: &Connection,
    w: &ImportPlanOfferWrite,
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
            w.plan_id.clone(),
            w.destination.clone(),
            w.offer_id.clone(),
            w.source_id.clone(),
            w.offer_type.clone(),
            w.title.clone(),
            w.price_per_person,
            w.currency.clone(),
            w.availability.clone(),
            w.url.clone(),
            w.scraped_at.clone(),
            w.price_total,
            now_db.to_string(),
        ],
    )
    .await
    .map_err(|e| e.to_string())?;

    // includes → plan_offer_includes child rows
    for (ii, item) in w.includes.iter().enumerate() {
        conn.execute(
            "INSERT INTO plan_offer_includes \
                (plan_id, destination, offer_id, sort_order, item) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            libsql::params![
                w.plan_id.clone(),
                w.destination.clone(),
                w.offer_id.clone(),
                ii as i64,
                item.clone(),
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
    }

    // flights
    if let Some(ref flight) = w.flights {
        for (dir, leg) in [
            ("outbound", &flight.outbound),
            ("return", &flight.return_leg),
        ] {
            conn.execute(
                "INSERT INTO plan_offer_flights \
                    (plan_id, destination, offer_id, direction, flight_number, airline, \
                     airline_code, departure_code, departure_time, arrival_code, arrival_time, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                libsql::params![
                    w.plan_id.clone(),
                    w.destination.clone(),
                    w.offer_id.clone(),
                    dir.to_string(),
                    leg.flight_number.clone(),
                    flight.airline.clone(),
                    flight.airline_code.clone(),
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
    if let Some(ref hotel) = w.hotel {
        conn.execute(
            "INSERT INTO plan_offer_hotels \
                (plan_id, destination, offer_id, name, slug, area, star_rating, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            libsql::params![
                w.plan_id.clone(),
                w.destination.clone(),
                w.offer_id.clone(),
                hotel.name.clone(),
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
                    w.plan_id.clone(),
                    w.destination.clone(),
                    w.offer_id.clone(),
                    ai as i64,
                    line.clone(),
                ],
            )
            .await
            .map_err(|e| e.to_string())?;
        }
    }

    // date_pricing
    for dp in &w.date_pricing {
        conn.execute(
            "INSERT INTO plan_offer_date_pricing \
                (plan_id, destination, offer_id, date, price, availability, seats_remaining, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            libsql::params![
                w.plan_id.clone(),
                w.destination.clone(),
                w.offer_id.clone(),
                dp.date.clone(),
                dp.price,
                dp.availability.clone(),
                dp.seats_remaining,
                now_db.to_string(),
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
    }

    // best_value
    if let Some(ref bv) = w.best_value {
        conn.execute(
            "INSERT INTO plan_offer_best_value \
                (plan_id, destination, offer_id, best_date, best_price, currency, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            libsql::params![
                w.plan_id.clone(),
                w.destination.clone(),
                w.offer_id.clone(),
                bv.date.clone(),
                bv.price,
                bv.currency.clone(),
                now_db.to_string(),
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// INSERT OR IGNORE into plan_offer_provenance.
pub async fn insert_import_provenance(
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

/// Payload for one tour-group bridged offer + its group_meta (shaping-adopt bridge).
#[derive(Debug, Clone)]
pub struct BridgeOfferWrite {
    pub plan_id: String,
    pub destination: String,
    pub offer_id: String,
    pub source_id: String,
    pub title: String,
    pub price_per_person: i64,
    pub departure_status: Option<String>,
    pub url: String,
    pub scraped_at: String,
    pub duration_days: i64,
    pub meals_included_count: Option<i64>,
    pub seats_available: Option<i64>,
    pub min_group_size: Option<i64>,
    pub group_size_cap: Option<i64>,
    pub source_run_id: String,
}

/// Insert one bridged group-tour offer + its group_meta. BYTE-IDENTICAL to the
/// inline SQL in tour_group_bridge.rs::bridge_audit_set.
pub async fn insert_bridge_offer(
    conn: &Connection,
    w: &BridgeOfferWrite,
) -> Result<(), String> {
    conn.execute(
        "INSERT OR REPLACE INTO plan_offers \
            (plan_id, destination, id, source_id, type, title, price_per_person, currency, \
             availability, url, scraped_at, duration_days, package_subtype) \
         VALUES (?1, ?2, ?3, ?4, 'package', ?5, ?6, 'TWD', ?7, ?8, ?9, ?10, 'group_tour')",
        libsql::params![
            w.plan_id.clone(),
            w.destination.clone(),
            w.offer_id.clone(),
            w.source_id.clone(),
            w.title.clone(),
            w.price_per_person,
            w.departure_status.clone(),
            w.url.clone(),
            w.scraped_at.clone(),
            w.duration_days,
        ],
    )
    .await
    .map_err(|e| format!("plan_offers insert failed for {}: {e}", w.offer_id))?;

    conn.execute(
        "INSERT OR REPLACE INTO plan_offer_group_meta \
            (plan_id, destination, offer_id, meals_included_count, departure_status, \
             seats_available, min_group_size, group_size_cap, source_offer_run_id, source_offer_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        libsql::params![
            w.plan_id.clone(),
            w.destination.clone(),
            w.offer_id.clone(),
            w.meals_included_count,
            w.departure_status.clone(),
            w.seats_available,
            w.min_group_size,
            w.group_size_cap,
            w.source_run_id.clone(),
            w.offer_id.clone(),
        ],
    )
    .await
    .map_err(|e| format!("plan_offer_group_meta insert failed for {}: {e}", w.offer_id))?;

    Ok(())
}

/// INSERT into plan_offer_warnings (warning_type hardcoded 'parse', offer_id omitted=NULL).
pub async fn insert_warning(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    message: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO plan_offer_warnings \
            (plan_id, destination, warning_type, message) \
         VALUES (?1, ?2, 'parse', ?3)",
        libsql::params![plan_id.to_string(), dest.to_string(), message.to_string(),],
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}
