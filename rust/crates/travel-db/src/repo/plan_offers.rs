//! `plan_offers` family domain writes for `promote-offers` (and future
//! `import_offers`/`select_offer` DAL migrations).
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
    pub flights: Option<[PlanOfferFlightWrite; 2]>,
    pub includes: Vec<String>,
}

/// One hotel child row (written only when `hotel` is `Some`).
#[derive(Debug, Clone)]
pub struct PlanOfferHotelWrite {
    pub name: String,
    pub area: Option<String>,
}

/// One flight leg child row (written as a pair when `flights` is `Some`).
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

    // plan_offer_flights — only if BOTH legs present (no fabrication).
    if let Some(ref legs) = write.flights {
        for leg in legs {
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

/// Upsert one `process_statuses` row (P3_4 → researched transition, etc.).
pub async fn upsert_process_status(
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