//! Plan-reader reads for `plan::load` (the `status`/`itinerary`/`bookings`/`transport` views).
//!
//! Owns the SQL + row mapping for the 16 normalized tables the reader assembles; every query is
//! bound on `?1` plan_id (+ `?2` destination, + `?3` offer_id where applicable) — no string
//! interpolation. The CLI keeps the `PlanView` assembly (grouping, HashMap inserts, find-by-day).

use libsql::Connection;

fn pd(plan_id: &str, destination: &str) -> Vec<libsql::Value> {
    vec![
        libsql::Value::Text(plan_id.to_string()),
        libsql::Value::Text(destination.to_string()),
    ]
}

/// `(schema_version, active_destination)` from `plan_metadata`; `None` if the plan_id is unknown.
pub async fn metadata(
    conn: &Connection,
    plan_id: &str,
) -> Result<Option<(String, String)>, String> {
    let mut rows = conn
        .query(
            "SELECT schema_version, active_destination FROM plan_metadata WHERE plan_id = ?1",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_metadata: {e}"))?;
    let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("plan_metadata row: {e}"))?
    else {
        return Ok(None);
    };
    Ok(Some((r.get(0).unwrap_or_default(), r.get(1).unwrap_or_default())))
}

/// `(process_id, status)` pairs from `process_statuses`.
pub async fn process_statuses(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<(String, String)>, String> {
    let mut rows = conn
        .query(
            "SELECT process_id, status FROM process_statuses \
             WHERE plan_id = ?1 AND destination = ?2",
            pd(plan_id, destination),
        )
        .await
        .map_err(|e| format!("process_statuses: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("process_statuses row: {e}"))?
    {
        out.push((r.get(0).unwrap_or_default(), r.get(1).unwrap_or_default()));
    }
    Ok(out)
}

/// `(process_id, dirty)` pairs from `cascade_dirty_flags` (dirty as 0/1).
pub async fn cascade_dirty_flags(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<(String, i64)>, String> {
    let mut rows = conn
        .query(
            "SELECT process_id, dirty FROM cascade_dirty_flags \
             WHERE plan_id = ?1 AND destination = ?2",
            pd(plan_id, destination),
        )
        .await
        .map_err(|e| format!("cascade_dirty_flags: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("cascade_dirty_flags row: {e}"))?
    {
        out.push((r.get(0).unwrap_or_default(), r.get(1).unwrap_or(0)));
    }
    Ok(out)
}

/// `(start_date, end_date, days)` from `date_anchors` for the destination; `None` if absent.
pub async fn date_anchor(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Option<(String, String, i64)>, String> {
    let mut rows = conn
        .query(
            "SELECT destination, start_date, end_date, days FROM date_anchors \
             WHERE plan_id = ?1 AND destination = ?2",
            pd(plan_id, destination),
        )
        .await
        .map_err(|e| format!("date_anchors: {e}"))?;
    let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("date_anchors row: {e}"))?
    else {
        return Ok(None);
    };
    Ok(Some((
        r.get(1).unwrap_or_default(),
        r.get(2).unwrap_or_default(),
        r.get(3).unwrap_or(0),
    )))
}

#[derive(Debug, Clone)]
pub struct FlightLegRow {
    pub direction: String,
    pub flight_number: String,
    pub airline: String,
    pub airline_code: String,
    pub departure_code: String,
    pub departure_terminal: String,
    pub departure_time: String,
    pub arrival_code: String,
    pub arrival_terminal: String,
    pub arrival_time: String,
    pub flight_date: String,
    pub booked_date: String,
}

pub async fn flight_legs(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<FlightLegRow>, String> {
    let mut rows = conn
        .query(
            "SELECT direction, leg_order, flight_number, airline, airline_code, \
                    departure_code, departure_terminal, departure_time, \
                    arrival_code, arrival_terminal, arrival_time, flight_date, booked_date \
             FROM flight_legs \
             WHERE plan_id = ?1 AND destination = ?2 \
             ORDER BY direction, leg_order",
            pd(plan_id, destination),
        )
        .await
        .map_err(|e| format!("flight_legs: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().await.map_err(|e| format!("flight_legs row: {e}"))? {
        out.push(FlightLegRow {
            direction: r.get(0).unwrap_or_default(),
            flight_number: r.get(2).unwrap_or_default(),
            airline: r.get(3).unwrap_or_default(),
            airline_code: r.get(4).unwrap_or_default(),
            departure_code: r.get(5).unwrap_or_default(),
            departure_terminal: r.get(6).unwrap_or_default(),
            departure_time: r.get(7).unwrap_or_default(),
            arrival_code: r.get(8).unwrap_or_default(),
            arrival_terminal: r.get(9).unwrap_or_default(),
            arrival_time: r.get(10).unwrap_or_default(),
            flight_date: r.get(11).unwrap_or_default(),
            booked_date: r.get(12).unwrap_or_default(),
        });
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct TransferRow {
    pub direction: String,
    pub status: String,
    pub selected_title: String,
    pub selected_route: String,
    pub selected_duration_min: Option<i64>,
    pub selected_price_yen: Option<i64>,
    pub selected_schedule: String,
    pub selected_booking_url: String,
    pub selected_notes: String,
    pub selected_id: String,
}

pub async fn airport_transfers(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<TransferRow>, String> {
    let mut rows = conn
        .query(
            "SELECT direction, status, selected_title, selected_route, \
                    selected_duration_min, selected_price_yen, selected_schedule, \
                    selected_booking_url, selected_notes, selected_id \
             FROM airport_transfers \
             WHERE plan_id = ?1 AND destination = ?2",
            pd(plan_id, destination),
        )
        .await
        .map_err(|e| format!("airport_transfers: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("airport_transfers row: {e}"))?
    {
        out.push(TransferRow {
            direction: r.get(0).unwrap_or_default(),
            status: r.get(1).unwrap_or_default(),
            selected_title: r.get(2).unwrap_or_default(),
            selected_route: r.get(3).unwrap_or_default(),
            selected_duration_min: r.get(4).ok(),
            selected_price_yen: r.get(5).ok(),
            selected_schedule: r.get(6).unwrap_or_default(),
            selected_booking_url: r.get(7).unwrap_or_default(),
            selected_notes: r.get(8).unwrap_or_default(),
            selected_id: r.get(9).unwrap_or_default(),
        });
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct TransferCandidateRow {
    pub direction: String,
    pub candidate_id: String,
    pub title: String,
    pub route: String,
    pub duration_min: Option<i64>,
    pub price_yen: Option<i64>,
    pub schedule: String,
    pub booking_url: String,
    pub notes: String,
}

pub async fn transfer_candidates(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<TransferCandidateRow>, String> {
    let mut rows = conn
        .query(
            "SELECT direction, candidate_id, title, route, duration_min, \
                    price_yen, schedule, booking_url, notes, sort_order \
             FROM airport_transfer_candidates \
             WHERE plan_id = ?1 AND destination = ?2 \
             ORDER BY direction, sort_order",
            pd(plan_id, destination),
        )
        .await
        .map_err(|e| format!("airport_transfer_candidates: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("airport_transfer_candidates row: {e}"))?
    {
        out.push(TransferCandidateRow {
            direction: r.get(0).unwrap_or_default(),
            candidate_id: r.get(1).unwrap_or_default(),
            title: r.get(2).unwrap_or_default(),
            route: r.get(3).unwrap_or_default(),
            duration_min: r.get(4).ok(),
            price_yen: r.get(5).ok(),
            schedule: r.get(6).unwrap_or_default(),
            booking_url: r.get(7).unwrap_or_default(),
            notes: r.get(8).unwrap_or_default(),
        });
    }
    Ok(out)
}

/// Hotel name for the destination; `None`/empty if absent.
pub async fn hotel_name(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Option<String>, String> {
    let mut rows = conn
        .query(
            "SELECT name FROM hotels WHERE plan_id = ?1 AND destination = ?2",
            pd(plan_id, destination),
        )
        .await
        .map_err(|e| format!("hotels: {e}"))?;
    let Some(r) = rows.next().await.map_err(|e| format!("hotels row: {e}"))? else {
        return Ok(None);
    };
    Ok(Some(r.get(0).unwrap_or_default()))
}

/// Hotel access lines (ordered by sort_order).
pub async fn hotel_access_lines(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<String>, String> {
    let mut rows = conn
        .query(
            "SELECT sort_order, line FROM hotel_access_lines \
             WHERE plan_id = ?1 AND destination = ?2 \
             ORDER BY sort_order",
            pd(plan_id, destination),
        )
        .await
        .map_err(|e| format!("hotel_access_lines: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("hotel_access_lines row: {e}"))?
    {
        out.push(r.get(1).unwrap_or_default());
    }
    Ok(out)
}

/// `(selected_offer_id, selected_date, selected_at)` from `plan_offer_selection`; `None` if absent.
pub async fn offer_selection(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Option<(String, String, String)>, String> {
    let mut rows = conn
        .query(
            "SELECT selected_offer_id, selected_date, selected_at \
             FROM plan_offer_selection \
             WHERE plan_id = ?1 AND destination = ?2",
            pd(plan_id, destination),
        )
        .await
        .map_err(|e| format!("plan_offer_selection: {e}"))?;
    let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("plan_offer_selection row: {e}"))?
    else {
        return Ok(None);
    };
    Ok(Some((
        r.get(0).unwrap_or_default(),
        r.get(1).unwrap_or_default(),
        r.get(2).unwrap_or_default(),
    )))
}

/// `plan_offer_includes` items for the selected offer (ordered by sort_order).
pub async fn offer_includes(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    offer_id: &str,
) -> Result<Vec<String>, String> {
    let mut rows = conn
        .query(
            "SELECT sort_order, item FROM plan_offer_includes \
             WHERE plan_id = ?1 AND destination = ?2 AND offer_id = ?3 \
             ORDER BY sort_order",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                offer_id.to_string()
            ],
        )
        .await
        .map_err(|e| format!("plan_offer_includes: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("plan_offer_includes row: {e}"))?
    {
        out.push(r.get(1).unwrap_or_default());
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct DayRow {
    pub day_number: i64,
    pub date: String,
    pub theme: String,
    pub day_type: String,
    pub weather_label: String,
    pub temp_low_c: f64,
    pub temp_high_c: f64,
    pub precipitation_pct: f64,
    pub weather_code: i64,
    pub weather_source_id: String,
    pub weather_sourced_at: String,
}

pub async fn days(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<DayRow>, String> {
    let mut rows = conn
        .query(
            "SELECT day_number, date, theme, day_type, weather_label, temp_low_c, \
                    temp_high_c, precipitation_pct, weather_code, weather_source_id, \
                    weather_sourced_at \
             FROM days \
             WHERE plan_id = ?1 AND destination = ?2 \
             ORDER BY day_number",
            pd(plan_id, destination),
        )
        .await
        .map_err(|e| format!("days: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().await.map_err(|e| format!("days row: {e}"))? {
        out.push(DayRow {
            day_number: r.get(0).unwrap_or(0),
            date: r.get(1).unwrap_or_default(),
            theme: r.get(2).unwrap_or_default(),
            day_type: r.get(3).unwrap_or_default(),
            weather_label: r.get(4).unwrap_or_default(),
            temp_low_c: r.get::<f64>(5).unwrap_or(0.0),
            temp_high_c: r.get::<f64>(6).unwrap_or(0.0),
            precipitation_pct: r.get::<f64>(7).unwrap_or(0.0),
            weather_code: r.get::<i64>(8).unwrap_or(0),
            weather_source_id: r.get(9).unwrap_or_default(),
            weather_sourced_at: r.get(10).unwrap_or_default(),
        });
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct TimesOfDayRow {
    pub day_number: i64,
    pub session_type: String,
    pub time_range_start: String,
    pub time_range_end: String,
    pub transit_notes: String,
    pub focus: String,
}

pub async fn timesofday(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<TimesOfDayRow>, String> {
    let mut rows = conn
        .query(
            "SELECT day_number, session_type, time_range_start, time_range_end, \
                    transit_notes, focus \
             FROM timesofday \
             WHERE plan_id = ?1 AND destination = ?2",
            pd(plan_id, destination),
        )
        .await
        .map_err(|e| format!("timesofday: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().await.map_err(|e| format!("timesofday row: {e}"))? {
        out.push(TimesOfDayRow {
            day_number: r.get(0).unwrap_or(0),
            session_type: r.get(1).unwrap_or_default(),
            time_range_start: r.get(2).unwrap_or_default(),
            time_range_end: r.get(3).unwrap_or_default(),
            transit_notes: r.get(4).unwrap_or_default(),
            focus: r.get(5).unwrap_or_default(),
        });
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct ActivityRow {
    pub day_number: i64,
    pub session_type: String,
    pub sort_order: i64,
    pub title: String,
    pub booking_required: i64,
    pub booking_status: String,
    pub booking_ref: String,
    pub is_fixed_time: i64,
    pub start_time: String,
    pub end_time: String,
    pub book_by: String,
}

pub async fn activities(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<ActivityRow>, String> {
    let mut rows = conn
        .query(
            "SELECT day_number, session_type, sort_order, title, booking_required, \
                    booking_status, booking_ref, is_fixed_time, start_time, end_time, book_by \
             FROM activities \
             WHERE plan_id = ?1 AND destination = ?2 \
             ORDER BY day_number, session_type, sort_order",
            pd(plan_id, destination),
        )
        .await
        .map_err(|e| format!("activities: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().await.map_err(|e| format!("activities row: {e}"))? {
        out.push(ActivityRow {
            day_number: r.get(0).unwrap_or(0),
            session_type: r.get(1).unwrap_or_default(),
            sort_order: r.get(2).unwrap_or(0),
            title: r.get(3).unwrap_or_default(),
            booking_required: r.get::<i64>(4).unwrap_or(0),
            booking_status: r.get(5).unwrap_or_default(),
            booking_ref: r.get(6).unwrap_or_default(),
            is_fixed_time: r.get::<i64>(7).unwrap_or(0),
            start_time: r.get(8).unwrap_or_default(),
            end_time: r.get(9).unwrap_or_default(),
            book_by: r.get(10).unwrap_or_default(),
        });
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct MealRow {
    pub day_number: i64,
    pub session_type: String,
    pub meal: String,
}

pub async fn session_meals(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<MealRow>, String> {
    let mut rows = conn
        .query(
            "SELECT day_number, session_type, sort_order, meal \
             FROM session_meals \
             WHERE plan_id = ?1 AND destination = ?2 \
             ORDER BY day_number, session_type, sort_order",
            pd(plan_id, destination),
        )
        .await
        .map_err(|e| format!("session_meals: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("session_meals row: {e}"))?
    {
        out.push(MealRow {
            day_number: r.get(0).unwrap_or(0),
            session_type: r.get(1).unwrap_or_default(),
            meal: r.get(3).unwrap_or_default(),
        });
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct RouteSegmentRow {
    pub day_number: i64,
    pub from_place: String,
    pub to_place: String,
    pub mode: String,
    pub duration_min: Option<i64>,
    pub notes: Option<String>,
    pub start_time: Option<String>,
}

pub async fn day_route_segments(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<RouteSegmentRow>, String> {
    let mut rows = conn
        .query(
            "SELECT day_number, sort_order, from_place, to_place, mode, \
                    duration_min, notes, start_time \
             FROM day_route_segments \
             WHERE plan_id = ?1 AND destination = ?2 \
             ORDER BY day_number, sort_order",
            pd(plan_id, destination),
        )
        .await
        .map_err(|e| format!("day_route_segments: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("day_route_segments row: {e}"))?
    {
        out.push(RouteSegmentRow {
            day_number: r.get(0).unwrap_or(0),
            from_place: r.get(2).unwrap_or_default(),
            to_place: r.get(3).unwrap_or_default(),
            mode: r.get(4).unwrap_or_default(),
            duration_min: r.get::<Option<i64>>(5).ok().flatten(),
            notes: r.get::<Option<String>>(6).ok().flatten(),
            start_time: r.get::<Option<String>>(7).ok().flatten(),
        });
    }
    Ok(out)
}
