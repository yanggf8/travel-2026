//! `flight_legs` domain writes for `set-flight`.
//!
//! DAL boundary: owns the leg upsert SQL. The audit triad
//! (`plan_events`/`plan_event_data`/`operation_runs`/`plans.version`) stays in `travel-cli`
//! (`cascade::common`). The caller decides which `(column, value)` pairs map from which input
//! fields (a CLI concern); this just writes them.

use libsql::Connection;

/// Upsert a single flight leg keyed on its PK `(plan_id, destination, direction, leg_order=0)`.
///
/// `cols` are `(column_name, value)` pairs the caller wants to write — column names are fixed
/// schema literals (never user input). `updated_at = now_db` is appended automatically. A plain
/// UPDATE would silently no-op on a booking-first plan that never seeded a skeleton leg, so this
/// INSERTs the row and, on conflict, re-applies the provided columns.
///
/// No-op when `cols` is empty (nothing to write besides the implicit `updated_at`, which would
/// touch nothing meaningful — callers only invoke this when they have fields to set).
pub async fn upsert_leg(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    direction: &str,
    cols: &[(&str, String)],
    now_db: &str,
) -> Result<(), String> {
    // Provided columns + the implicit updated_at.
    let mut col_names: Vec<&str> = cols.iter().map(|(c, _)| *c).collect();
    let mut vals: Vec<String> = cols.iter().map(|(_, v)| v.clone()).collect();
    col_names.push("updated_at");
    vals.push(now_db.to_string());

    // INSERT row = PK (plan_id, destination, direction, leg_order=0) + provided columns.
    let mut insert_cols: Vec<String> = vec![
        "plan_id".into(),
        "destination".into(),
        "direction".into(),
        "leg_order".into(),
    ];
    insert_cols.extend(col_names.iter().map(|c| c.to_string()));

    let mut params: Vec<libsql::Value> = vec![
        libsql::Value::Text(plan_id.to_string()),
        libsql::Value::Text(destination.to_string()),
        libsql::Value::Text(direction.to_string()),
        libsql::Value::Integer(0),
    ];
    params.extend(vals.iter().map(|v| libsql::Value::Text(v.clone())));

    let insert_ph: Vec<String> = (1..=insert_cols.len()).map(|n| format!("?{n}")).collect();

    // DO UPDATE SET re-applies the provided columns with a fresh copy of the values, numbered
    // after the INSERT params.
    let base = params.len();
    let update_sets: Vec<String> = col_names
        .iter()
        .enumerate()
        .map(|(idx, c)| format!("{c} = ?{}", base + 1 + idx))
        .collect();
    params.extend(vals.iter().map(|v| libsql::Value::Text(v.clone())));

    let sql = format!(
        "INSERT INTO flight_legs ({}) VALUES ({}) \
         ON CONFLICT(plan_id, destination, direction, leg_order) DO UPDATE SET {}",
        insert_cols.join(", "),
        insert_ph.join(", "),
        update_sets.join(", "),
    );
    conn.execute(&sql, params)
        .await
        .map_err(|e| format!("flight_legs upsert failed: {e}"))?;
    Ok(())
}

/// One flight leg from a chosen package offer (for `select-offer`'s populate cascade).
#[derive(Debug, Clone)]
pub struct OfferFlightLegWrite {
    pub direction: String,
    pub flight_number: Option<String>,
    pub departure_code: Option<String>,
    pub departure_time: Option<String>,
    pub arrival_code: Option<String>,
    pub arrival_time: Option<String>,
}

/// Replace ALL flight legs for `(plan_id, destination)` with the chosen offer's legs — the
/// `select-offer` populate cascade. DELETE-then-`INSERT OR REPLACE` per leg, byte-identical to
/// the SQL that was inline in `cascade::select_offer::rewrite_flight_legs`: fixed 19-column
/// layout with `leg_order=0`, airline/airline_code from the offer (parent), per-leg
/// number/dep+arr code+time, and explicit NULLs for departure_airport/terminal,
/// arrival_airport/terminal, and flight_date.
#[allow(clippy::too_many_arguments)]
pub async fn replace_from_offer(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    legs: &[OfferFlightLegWrite],
    airline: Option<&str>,
    airline_code: Option<&str>,
    populated_from: &str,
    booked_date: &str,
    now_db: &str,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM flight_legs WHERE plan_id = ?1 AND destination = ?2",
        libsql::params![plan_id.to_string(), destination.to_string()],
    )
    .await
    .map_err(|e| e.to_string())?;
    for leg in legs {
        conn.execute(
            "INSERT OR REPLACE INTO flight_legs \
                (plan_id, destination, direction, leg_order, flight_number, \
                 airline, airline_code, departure_airport, departure_code, \
                 departure_terminal, departure_time, arrival_airport, \
                 arrival_code, arrival_terminal, arrival_time, flight_date, \
                 populated_from, booked_date, updated_at) \
             VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6, NULL, ?7, NULL, ?8, NULL, ?9, NULL, ?10, NULL, ?11, ?12, ?13)",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                leg.direction.clone(),
                leg.flight_number.clone(),
                airline.map(|s| s.to_string()),
                airline_code.map(|s| s.to_string()),
                leg.departure_code.clone(),
                leg.departure_time.clone(),
                leg.arrival_code.clone(),
                leg.arrival_time.clone(),
                populated_from.to_string(),
                booked_date.to_string(),
                now_db.to_string()
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
