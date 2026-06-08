// Minimal-slice plan reader for `travel status` (Phase 4 read foundation).
//
// Reads ONLY the 14 tables `status.ts` actually touches and assembles a
// `PlanView` struct that mirrors the slice of the assembled plan object the
// TS status formatter uses. No JSON in Turso columns: list-shaped fields
// (access lines, transfer candidates, offer includes) are read from
// normalized child tables or `*_text` columns; scalars come from typed
// columns. This is intentionally extensible — the next read view (itinerary,
// transport, bookings, view-prices) can re-use `PlanView` and add new
// tables / fields without changing the dispatch surface.
//
// Tables covered (all parameterized on plan_id):
//   1.  plan_metadata
//   2.  process_statuses
//   3.  cascade_dirty_flags
//   4.  date_anchors
//   5.  flight_legs
//   6.  airport_transfers            (selected_* scalar cols)
//   7.  airport_transfer_candidates  (child rows)
//   8.  hotels
//   9.  hotel_access_lines           (child rows)
//   10. plan_offer_selection
//   11. plan_offer_includes           (child rows, joined via selection)
//   12. days
//   13. timesofday                   (time_range_start/end)
//   14. activities
//
// Tables NOT covered (extensibility hooks for the next view port):
//   plan_offers, plan_offer_flights, plan_offer_hotels, plan_offer_date_pricing,
//   plan_offer_best_value, plan_offer_warnings, plan_offer_provenance,
//   plan_offer_hotel_access, plan_event_data, plan_events,
//   event_log_state, event_log_global_processes, event_log_destinations,
//   event_log_dest_processes, event_log_next_actions,
//   cascade_triggers, cascade_trigger_resets, cascade_trigger_populate_map,
//   cascade_global_state, plan_root_date_anchor, plan_schema_contract (+_nodes),
//   plan_process_precedence (+_entries), plan_date_anchor_flex_dates,
//   plan_budget, day_route_segments, day_landmarks, session_activities_zh,
//   session_meals, activity_tags, transportation_extras, transport_extra_candidates,
//   accommodation_location_zone, location_zone_candidates, itinerary_metadata,
//   itinerary_transit_key_lines, destination_*, plans (versions),
//   offers, destinations, events, bookings_*, hotels extra cols (name_zh),
//   holiday_calendar, ota_*, etc.

use crate::db;
use std::collections::HashMap;

/// Assembled plan slice for `travel status` (and the next read views).
///
/// `#[allow(dead_code)]` on individual fields marks the public PlanView
/// surface for the next view port (itinerary / transport / bookings /
/// view-prices) — the status formatter does not yet consume these fields
/// but the next port will. Each allow is field-level (not module-level)
/// so the rest of the module is held to a no-warnings bar.
#[derive(Debug, Default)]
pub struct PlanView {
    pub active_destination: String,
    #[allow(dead_code)] // surfaced for the next view port (schema checks, header)
    pub schema_version: String,
    pub process_status: HashMap<String, String>,                // process_id -> status
    pub dirty_flags: HashMap<String, bool>,                      // process_id -> dirty
    pub dates: Option<DateAnchor>,                              // one entry per destination
    pub flight: Option<FlightSummary>,                          // outbound + return
    pub transfers: HashMap<String, TransferDir>,                // direction -> TransferDir
    pub hotel: Option<HotelSummary>,
    /// Selected offer from `plan_offer_selection`. The TS `status.ts` does
    /// not surface this on a fresh read (the in-memory `chosen_offer` is
    /// only populated by the mutator), so status.rs currently does not
    /// render it. Surfaced here for the next view port.
    #[allow(dead_code)]
    pub selected_offer: Option<SelectedOffer>,
    /// Items from `plan_offer_includes` joined via the selected offer.
    /// Same quirk as `selected_offer` — surfaced for the next view port.
    #[allow(dead_code)]
    pub offer_includes: Vec<String>,
    pub days: Vec<DayView>,                                      // ordered by day_number
}

#[derive(Debug, Clone)]
pub struct DateAnchor {
    pub start_date: String,
    pub end_date: String,
    pub days: i64,
}

#[derive(Debug, Clone)]
pub struct FlightSummary {
    pub airline: String,
    pub airline_code: String,
    /// `flight_legs.booked_date` — surfaced for the next view port
    /// (transport / bookings).
    #[allow(dead_code)]
    pub booked_date: String,
    pub outbound: Option<FlightLegView>,
    pub inbound: Option<FlightLegView>,
}

#[derive(Debug, Clone)]
pub struct FlightLegView {
    pub flight_number: String,
    pub departure_airport_code: String,
    pub departure_terminal: String,
    pub departure_time: String,
    pub arrival_airport_code: String,
    pub arrival_terminal: String,
    pub arrival_time: String,
    /// `flight_legs.flight_date` — surfaced for the next view port
    /// (transport) so the per-leg date is available without re-reading.
    #[allow(dead_code)]
    pub date: String,
}

#[derive(Debug, Clone)]
pub struct TransferDir {
    pub status: String,
    pub selected: Option<TransferOption>,
    pub candidates: Vec<TransferOption>,
}

#[derive(Debug, Clone)]
pub struct TransferOption {
    /// `airport_transfer_candidates.candidate_id` / `airport_transfers.selected_id`.
    /// Surfaced for the next view port (itinerary cross-refs, deep links).
    #[allow(dead_code)]
    pub id: String,
    pub title: String,
    pub route: String,
    pub duration_min: Option<i64>,
    pub price_yen: Option<i64>,
    pub schedule: String,
    /// `selected_booking_url` / candidate `booking_url`. Surfaced for the
    /// next view port (transport deep links).
    #[allow(dead_code)]
    pub booking_url: String,
    /// `selected_notes` / candidate `notes`. Surfaced for the next view
    /// port (transport detail).
    #[allow(dead_code)]
    pub notes: String,
}

#[derive(Debug, Clone)]
pub struct HotelSummary {
    pub name: String,
    pub access: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SelectedOffer {
    /// See `PlanView::selected_offer` — surfaced for the next view port
    /// (Selected Offer block on the rendered view).
    #[allow(dead_code)]
    pub id: String,
    #[allow(dead_code)]
    pub date: String,
    #[allow(dead_code)]
    pub selected_at: String,
}

#[derive(Debug, Clone)]
pub struct DayView {
    pub day_number: i64,
    /// `days.date` — surfaced for the next view port (itinerary header).
    #[allow(dead_code)]
    pub date: String,
    /// `days.theme` — surfaced for the next view port (transport view day
    /// header `Day N (date) - <theme>` suffix).
    #[allow(dead_code)]
    pub theme: String,
    pub sessions: HashMap<String, SessionView>, // "morning"|"noon"|"afternoon"|"evening"
}

#[derive(Debug, Clone)]
pub struct SessionView {
    pub time_range_start: String,
    pub time_range_end: String,
    /// `timesofday.transit_notes` — surfaced for the next view port
    /// (transport view per-session transit lines).
    #[allow(dead_code)]
    pub transit_notes: String,
    pub activities: Vec<ActivityView>,
}

#[derive(Debug, Clone)]
pub struct ActivityView {
    pub title: String,
    pub booking_required: bool,
    pub booking_status: String,
    pub booking_ref: String,
    pub is_fixed_time: bool,
    pub start_time: String,
    pub end_time: String,
    /// `activities.sort_order` — surfaced for the next view port (itinerary
    /// preserves DB order without re-sorting on the client).
    #[allow(dead_code)]
    pub sort_order: i64,
}

/// Load a `PlanView` for the given `plan_id`. Throws if `plan_metadata` has
/// no row for `plan_id` (matches the TS `[turso] Plan "X" not found in
/// normalized tables. Run migration first.` error message).
pub async fn load(plan_id: &str) -> Result<PlanView, String> {
    let conn = db::connect_read().await?;
    let esc = sql_quote(plan_id);

    // 1. plan_metadata
    let mut rows = conn
        .query(
            &format!("SELECT schema_version, active_destination FROM plan_metadata WHERE plan_id = '{esc}'"),
            (),
        )
        .await
        .map_err(|e| format!("plan_metadata: {e}"))?;
    let meta = rows
        .next()
        .await
        .map_err(|e| format!("plan_metadata row: {e}"))?
        .ok_or_else(|| {
            format!(
                "[turso] Plan \"{plan_id}\" not found in normalized tables. Run migration first."
            )
        })?;
    let schema_version: String = meta.get(0).unwrap_or_default();
    let active_destination: String = meta.get(1).unwrap_or_default();

    let mut view = PlanView {
        schema_version,
        active_destination: active_destination.clone(),
        ..Default::default()
    };

    // 2 + 3. process_statuses + cascade_dirty_flags keyed by process_id (for
    // the active destination; the TS only reads statuses for `dest`).
    let dest_esc = sql_quote(&active_destination);
    let mut rows = conn
        .query(
            &format!(
                "SELECT process_id, status FROM process_statuses \
                 WHERE plan_id = '{esc}' AND destination = '{dest_esc}'"
            ),
            (),
        )
        .await
        .map_err(|e| format!("process_statuses: {e}"))?;
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("process_statuses row: {e}"))?
    {
        let pid: String = r.get(0).unwrap_or_default();
        let st: String = r.get(1).unwrap_or_default();
        if !pid.is_empty() {
            view.process_status.insert(pid, st);
        }
    }
    let mut rows = conn
        .query(
            &format!(
                "SELECT process_id, dirty FROM cascade_dirty_flags \
                 WHERE plan_id = '{esc}' AND destination = '{dest_esc}'"
            ),
            (),
        )
        .await
        .map_err(|e| format!("cascade_dirty_flags: {e}"))?;
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("cascade_dirty_flags row: {e}"))?
    {
        let pid: String = r.get(0).unwrap_or_default();
        let dirty: i64 = r.get(1).unwrap_or(0);
        if !pid.is_empty() {
            view.dirty_flags.insert(pid, dirty != 0);
        }
    }

    // 4. date_anchors (one row per destination; we keep the one for the
    // active destination; status.ts calls `sm.getDateAnchor()` which uses
    // the active dest).
    let mut rows = conn
        .query(
            &format!(
                "SELECT destination, start_date, end_date, days FROM date_anchors \
                 WHERE plan_id = '{esc}' AND destination = '{dest_esc}'"
            ),
            (),
        )
        .await
        .map_err(|e| format!("date_anchors: {e}"))?;
    if let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("date_anchors row: {e}"))?
    {
        let start: String = r.get(1).unwrap_or_default();
        let end: String = r.get(2).unwrap_or_default();
        let days: i64 = r.get(3).unwrap_or(0);
        view.dates = Some(DateAnchor { start_date: start, end_date: end, days });
    }

    // 5. flight_legs (ordered by direction, leg_order; the assembler takes
    // direction="outbound"/"return" and assigns to outbound/return).
    let mut outbound: Option<FlightLegView> = None;
    let mut inbound: Option<FlightLegView> = None;
    let mut airline = String::new();
    let mut airline_code = String::new();
    let mut booked_date = String::new();
    let mut rows = conn
        .query(
            &format!(
                "SELECT direction, leg_order, flight_number, airline, airline_code, \
                        departure_code, departure_terminal, departure_time, \
                        arrival_code, arrival_terminal, arrival_time, flight_date, booked_date \
                 FROM flight_legs \
                 WHERE plan_id = '{esc}' AND destination = '{dest_esc}' \
                 ORDER BY direction, leg_order"
            ),
            (),
        )
        .await
        .map_err(|e| format!("flight_legs: {e}"))?;
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("flight_legs row: {e}"))?
    {
        let direction: String = r.get(0).unwrap_or_default();
        let flight_number: String = r.get(2).unwrap_or_default();
        let leg_airline: String = r.get(3).unwrap_or_default();
        let leg_airline_code: String = r.get(4).unwrap_or_default();
        if airline.is_empty() && !leg_airline.is_empty() {
            airline = leg_airline.clone();
        }
        if airline_code.is_empty() && !leg_airline_code.is_empty() {
            airline_code = leg_airline_code.clone();
        }
        if booked_date.is_empty() {
            let bd: String = r.get(12).unwrap_or_default();
            if !bd.is_empty() {
                booked_date = bd;
            }
        }
        let leg = FlightLegView {
            flight_number,
            departure_airport_code: r.get(5).unwrap_or_default(),
            departure_terminal: r.get(6).unwrap_or_default(),
            departure_time: r.get(7).unwrap_or_default(),
            arrival_airport_code: r.get(8).unwrap_or_default(),
            arrival_terminal: r.get(9).unwrap_or_default(),
            arrival_time: r.get(10).unwrap_or_default(),
            date: r.get(11).unwrap_or_default(),
        };
        match direction.as_str() {
            "outbound" if outbound.is_none() => outbound = Some(leg),
            "return" if inbound.is_none() => inbound = Some(leg),
            _ => {}
        }
    }
    if outbound.is_some() || inbound.is_some() {
        view.flight = Some(FlightSummary {
            airline,
            airline_code,
            booked_date,
            outbound,
            inbound,
        });
    }

    // 6. airport_transfers (one row per direction; selected_* scalar cols).
    let mut rows = conn
        .query(
            &format!(
                "SELECT direction, status, selected_title, selected_route, \
                        selected_duration_min, selected_price_yen, selected_schedule, \
                        selected_booking_url, selected_notes, selected_id \
                 FROM airport_transfers \
                 WHERE plan_id = '{esc}' AND destination = '{dest_esc}'"
            ),
            (),
        )
        .await
        .map_err(|e| format!("airport_transfers: {e}"))?;
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("airport_transfers row: {e}"))?
    {
        let direction: String = r.get(0).unwrap_or_default();
        let status: String = r.get(1).unwrap_or_default();
        let sel_title: String = r.get(2).unwrap_or_default();
        let sel_route: String = r.get(3).unwrap_or_default();
        let sel_dur: Option<i64> = r.get(4).ok();
        let sel_price: Option<i64> = r.get(5).ok();
        let sel_sched: String = r.get(6).unwrap_or_default();
        let sel_url: String = r.get(7).unwrap_or_default();
        let sel_notes: String = r.get(8).unwrap_or_default();
        let sel_id: String = r.get(9).unwrap_or_default();
        let selected = if !sel_title.is_empty() || !sel_id.is_empty() {
            Some(TransferOption {
                id: sel_id,
                title: sel_title,
                route: sel_route,
                duration_min: sel_dur,
                price_yen: sel_price,
                schedule: sel_sched,
                booking_url: sel_url,
                notes: sel_notes,
            })
        } else {
            None
        };
        view.transfers.insert(
            direction.clone(),
            TransferDir { status, selected, candidates: Vec::new() },
        );
    }

    // 7. airport_transfer_candidates (child rows, ordered by sort_order).
    let mut rows = conn
        .query(
            &format!(
                "SELECT direction, candidate_id, title, route, duration_min, \
                        price_yen, schedule, booking_url, notes, sort_order \
                 FROM airport_transfer_candidates \
                 WHERE plan_id = '{esc}' AND destination = '{dest_esc}' \
                 ORDER BY direction, sort_order"
            ),
            (),
        )
        .await
        .map_err(|e| format!("airport_transfer_candidates: {e}"))?;
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("airport_transfer_candidates row: {e}"))?
    {
        let direction: String = r.get(0).unwrap_or_default();
        let cand = TransferOption {
            id: r.get(1).unwrap_or_default(),
            title: r.get(2).unwrap_or_default(),
            route: r.get(3).unwrap_or_default(),
            duration_min: r.get(4).ok(),
            price_yen: r.get(5).ok(),
            schedule: r.get(6).unwrap_or_default(),
            booking_url: r.get(7).unwrap_or_default(),
            notes: r.get(8).unwrap_or_default(),
        };
        if let Some(td) = view.transfers.get_mut(&direction) {
            td.candidates.push(cand);
        }
    }

    // 8 + 9. hotels + hotel_access_lines.
    let mut rows = conn
        .query(
            &format!(
                "SELECT name FROM hotels \
                 WHERE plan_id = '{esc}' AND destination = '{dest_esc}'"
            ),
            (),
        )
        .await
        .map_err(|e| format!("hotels: {e}"))?;
    if let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("hotels row: {e}"))?
    {
        let name: String = r.get(0).unwrap_or_default();
        if !name.is_empty() {
            view.hotel = Some(HotelSummary { name, access: Vec::new() });
        }
    }
    let mut rows = conn
        .query(
            &format!(
                "SELECT sort_order, line FROM hotel_access_lines \
                 WHERE plan_id = '{esc}' AND destination = '{dest_esc}' \
                 ORDER BY sort_order"
            ),
            (),
        )
        .await
        .map_err(|e| format!("hotel_access_lines: {e}"))?;
    if let Some(h) = view.hotel.as_mut() {
        while let Some(r) = rows
            .next()
            .await
            .map_err(|e| format!("hotel_access_lines row: {e}"))?
        {
            let line: String = r.get(1).unwrap_or_default();
            if !line.is_empty() {
                h.access.push(line);
            }
        }
    }

    // 10 + 11. plan_offer_selection + plan_offer_includes.
    let mut rows = conn
        .query(
            &format!(
                "SELECT selected_offer_id, selected_date, selected_at \
                 FROM plan_offer_selection \
                 WHERE plan_id = '{esc}' AND destination = '{dest_esc}'"
            ),
            (),
        )
        .await
        .map_err(|e| format!("plan_offer_selection: {e}"))?;
    let mut selected_offer_id = String::new();
    if let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("plan_offer_selection row: {e}"))?
    {
        let id: String = r.get(0).unwrap_or_default();
        let date: String = r.get(1).unwrap_or_default();
        let at: String = r.get(2).unwrap_or_default();
        if !id.is_empty() {
            view.selected_offer = Some(SelectedOffer {
                id: id.clone(),
                date,
                selected_at: at,
            });
            selected_offer_id = id;
        }
    }
    if !selected_offer_id.is_empty() {
        let offer_esc = sql_quote(&selected_offer_id);
        let mut rows = conn
            .query(
                &format!(
                    "SELECT sort_order, item FROM plan_offer_includes \
                     WHERE plan_id = '{esc}' AND destination = '{dest_esc}' \
                       AND offer_id = '{offer_esc}' \
                     ORDER BY sort_order"
                ),
                (),
            )
            .await
            .map_err(|e| format!("plan_offer_includes: {e}"))?;
        while let Some(r) = rows
            .next()
            .await
            .map_err(|e| format!("plan_offer_includes row: {e}"))?
        {
            let item: String = r.get(1).unwrap_or_default();
            if !item.is_empty() {
                view.offer_includes.push(item);
            }
        }
    }

    // 12. days.
    let mut rows = conn
        .query(
            &format!(
                "SELECT day_number, date, theme FROM days \
                 WHERE plan_id = '{esc}' AND destination = '{dest_esc}' \
                 ORDER BY day_number"
            ),
            (),
        )
        .await
        .map_err(|e| format!("days: {e}"))?;
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("days row: {e}"))?
    {
        let day_number: i64 = r.get(0).unwrap_or(0);
        let date: String = r.get(1).unwrap_or_default();
        let theme: String = r.get(2).unwrap_or_default();
        view.days.push(DayView {
            day_number,
            date,
            theme,
            sessions: HashMap::new(),
        });
    }

    // 13. timesofday (key: day_number -> SessionView).
    let mut rows = conn
        .query(
            &format!(
                "SELECT day_number, session_type, time_range_start, time_range_end, transit_notes \
                 FROM timesofday \
                 WHERE plan_id = '{esc}' AND destination = '{dest_esc}'"
            ),
            (),
        )
        .await
        .map_err(|e| format!("timesofday: {e}"))?;
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("timesofday row: {e}"))?
    {
        let day_number: i64 = r.get(0).unwrap_or(0);
        let session_type: String = r.get(1).unwrap_or_default();
        if let Some(d) = view.days.iter_mut().find(|d| d.day_number == day_number) {
            d.sessions.insert(
                session_type,
                SessionView {
                    time_range_start: r.get(2).unwrap_or_default(),
                    time_range_end: r.get(3).unwrap_or_default(),
                    transit_notes: r.get(4).unwrap_or_default(),
                    activities: Vec::new(),
                },
            );
        }
    }

    // 14. activities (key: day_number, session_type -> activity rows; order
    // by sort_order for stable output when the TS iterates the array).
    let mut rows = conn
        .query(
            &format!(
                "SELECT day_number, session_type, sort_order, title, booking_required, \
                        booking_status, booking_ref, is_fixed_time, start_time, end_time \
                 FROM activities \
                 WHERE plan_id = '{esc}' AND destination = '{dest_esc}' \
                 ORDER BY day_number, session_type, sort_order"
            ),
            (),
        )
        .await
        .map_err(|e| format!("activities: {e}"))?;
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("activities row: {e}"))?
    {
        let day_number: i64 = r.get(0).unwrap_or(0);
        let session_type: String = r.get(1).unwrap_or_default();
        let sort_order: i64 = r.get(2).unwrap_or(0);
        let act = ActivityView {
            title: r.get(3).unwrap_or_default(),
            booking_required: r.get::<i64>(4).unwrap_or(0) != 0,
            booking_status: r.get(5).unwrap_or_default(),
            booking_ref: r.get(6).unwrap_or_default(),
            is_fixed_time: r.get::<i64>(7).unwrap_or(0) != 0,
            start_time: r.get(8).unwrap_or_default(),
            end_time: r.get(9).unwrap_or_default(),
            sort_order,
        };
        if let Some(d) = view.days.iter_mut().find(|d| d.day_number == day_number)
            && let Some(s) = d.sessions.get_mut(&session_type)
        {
            s.activities.push(act);
        }
    }

    Ok(view)
}

fn sql_quote(v: &str) -> String {
    v.replace('\'', "''")
}
