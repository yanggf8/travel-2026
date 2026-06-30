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
//   12. days                         (incl. weather_* scalars, day_type)
//   13. timesofday                   (incl. focus, transit_notes)
//   14. activities                   (incl. book_by)
//   15. session_meals                (child rows; ordered by sort_order)
//   16. day_route_segments           (child rows; ordered by day_number, sort_order)
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
//   plan_budget, day_landmarks, session_activities_zh,
//   activity_tags, transportation_extras, transport_extra_candidates,
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
    /// Per-day route segments from `day_route_segments` (keyed by
    /// `day_number`). Consumed by the itinerary view (per-day ROUTE
    /// block). `status.rs` does not surface it.
    #[allow(dead_code)]
    pub route_segments: HashMap<i32, Vec<RouteSegment>>,
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
    /// header `Day N (date) - <theme>` suffix; itinerary `Theme: <theme>` line).
    #[allow(dead_code)]
    pub theme: String,
    /// `days.day_type` — surfaced for the next view port (itinerary
    /// `Day N (date) ✈️ ARRIVAL|DEPARTURE` suffix).
    #[allow(dead_code)]
    pub day_type: String,
    /// `days.weather_*` scalars joined into a single struct. Surfaced for
    /// the next view port (itinerary weather line).
    #[allow(dead_code)]
    pub weather: Option<DayWeather>,
    pub sessions: HashMap<String, SessionView>, // "morning"|"noon"|"afternoon"|"evening"
}

#[derive(Debug, Clone)]
pub struct DayWeather {
    pub weather_label: String,
    pub temp_low_c: f64,
    pub temp_high_c: f64,
    pub precipitation_pct: f64,
    pub weather_code: i64,
    pub source_id: String,
    pub sourced_at: String,
}

#[derive(Debug, Clone)]
pub struct SessionView {
    pub time_range_start: String,
    pub time_range_end: String,
    /// `timesofday.transit_notes` — surfaced for the next view port
    /// (transport view per-session transit lines, itinerary per-session
    /// `    🚃 <notes>` line).
    #[allow(dead_code)]
    pub transit_notes: String,
    /// `timesofday.focus` — surfaced for the next view port (itinerary
    /// `【Session】 <focus>` line).
    #[allow(dead_code)]
    pub focus: String,
    /// Items from `session_meals` child table, ordered by `sort_order`.
    /// Surfaced for the next view port (itinerary `    🍽️  <meals>` line).
    #[allow(dead_code)]
    pub meals: Vec<String>,
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
    /// `activities.book_by` — surfaced for the next view port (itinerary
    /// per-activity ` (book by <date>)` suffix when `booking_status` is
    /// `pending`, and PENDING BOOKINGS section `(by <date>)` suffix).
    #[allow(dead_code)]
    pub book_by: String,
    /// `activities.sort_order` — surfaced for the next view port (itinerary
    /// preserves DB order without re-sorting on the client).
    #[allow(dead_code)]
    pub sort_order: i64,
}

#[derive(Debug, Clone)]
pub struct RouteSegment {
    pub from_place: String,
    pub to_place: String,
    pub mode: String,
    pub duration_min: Option<i64>,
    pub notes: Option<String>,
    /// `day_route_segments.start_time` — surfaced for the next view port
    /// (e.g. itinerary segment with explicit start time). Not used by the
    /// current itinerary view (we render by sort order).
    #[allow(dead_code)]
    pub start_time: Option<String>,
}

/// Load a `PlanView` for the given `plan_id`. Throws if `plan_metadata` has
/// no row for `plan_id` (matches the TS `[turso] Plan "X" not found in
/// normalized tables. Run migration first.` error message).
/// Fail loud if a `--dest <slug>` override does not match the plan's
/// `active_destination`. View commands (`itinerary`/`bookings`/`transport`)
/// load data keyed on `active_destination`; they used to PARSE `--dest` and
/// silently ignore it, so `itinerary --dest wrong_slug` quietly showed the
/// active destination instead. Until multi-destination plans exist, the honest
/// behavior is to reject a mismatching `--dest` rather than mislead. A matching
/// (or absent) `--dest` is a no-op.
pub fn assert_dest_matches(dest_opt: Option<&str>, active_destination: &str) -> Result<(), String> {
    if let Some(d) = dest_opt {
        if d != active_destination {
            return Err(format!(
                "--dest {d} does not match this plan's active destination ({active_destination}). \
                 View commands only render the active destination; drop --dest or pass --dest {active_destination}."
            ));
        }
    }
    Ok(())
}

pub async fn load(plan_id: &str) -> Result<PlanView, String> {
    use travel_db::repo::plan as repo;
    let conn = db::connect_read().await?;

    // 1. plan_metadata (fail loud if the plan is unknown — matches the TS message).
    let (schema_version, active_destination) = repo::metadata(&conn, plan_id).await?.ok_or_else(
        || format!("[turso] Plan \"{plan_id}\" not found in normalized tables. Run migration first."),
    )?;
    let dest = active_destination.as_str();

    let mut view = PlanView {
        schema_version,
        active_destination: active_destination.clone(),
        ..Default::default()
    };

    // 2 + 3. process_statuses + cascade_dirty_flags keyed by process_id (for
    // the active destination; the TS only reads statuses for `dest`).
    for (pid, st) in repo::process_statuses(&conn, plan_id, dest).await? {
        if !pid.is_empty() {
            view.process_status.insert(pid, st);
        }
    }
    for (pid, dirty) in repo::cascade_dirty_flags(&conn, plan_id, dest).await? {
        if !pid.is_empty() {
            view.dirty_flags.insert(pid, dirty != 0);
        }
    }

    // 4. date_anchors (one row per destination; we keep the one for the
    // active destination; status.ts calls `sm.getDateAnchor()` which uses
    // the active dest).
    if let Some((start_date, end_date, days)) = repo::date_anchor(&conn, plan_id, dest).await? {
        view.dates = Some(DateAnchor { start_date, end_date, days });
    }

    // 5. flight_legs (ordered by direction, leg_order; the assembler takes
    // direction="outbound"/"return" and assigns to outbound/return).
    let mut outbound: Option<FlightLegView> = None;
    let mut inbound: Option<FlightLegView> = None;
    let mut airline = String::new();
    let mut airline_code = String::new();
    let mut booked_date = String::new();
    for leg_row in repo::flight_legs(&conn, plan_id, dest).await? {
        if airline.is_empty() && !leg_row.airline.is_empty() {
            airline = leg_row.airline.clone();
        }
        if airline_code.is_empty() && !leg_row.airline_code.is_empty() {
            airline_code = leg_row.airline_code.clone();
        }
        if booked_date.is_empty() && !leg_row.booked_date.is_empty() {
            booked_date = leg_row.booked_date.clone();
        }
        let leg = FlightLegView {
            flight_number: leg_row.flight_number,
            departure_airport_code: leg_row.departure_code,
            departure_terminal: leg_row.departure_terminal,
            departure_time: leg_row.departure_time,
            arrival_airport_code: leg_row.arrival_code,
            arrival_terminal: leg_row.arrival_terminal,
            arrival_time: leg_row.arrival_time,
            date: leg_row.flight_date,
        };
        match leg_row.direction.as_str() {
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
    for t in repo::airport_transfers(&conn, plan_id, dest).await? {
        let selected = if !t.selected_title.is_empty() || !t.selected_id.is_empty() {
            Some(TransferOption {
                id: t.selected_id,
                title: t.selected_title,
                route: t.selected_route,
                duration_min: t.selected_duration_min,
                price_yen: t.selected_price_yen,
                schedule: t.selected_schedule,
                booking_url: t.selected_booking_url,
                notes: t.selected_notes,
            })
        } else {
            None
        };
        view.transfers.insert(
            t.direction,
            TransferDir { status: t.status, selected, candidates: Vec::new() },
        );
    }

    // 7. airport_transfer_candidates (child rows, ordered by sort_order).
    for c in repo::transfer_candidates(&conn, plan_id, dest).await? {
        let cand = TransferOption {
            id: c.candidate_id,
            title: c.title,
            route: c.route,
            duration_min: c.duration_min,
            price_yen: c.price_yen,
            schedule: c.schedule,
            booking_url: c.booking_url,
            notes: c.notes,
        };
        if let Some(td) = view.transfers.get_mut(&c.direction) {
            td.candidates.push(cand);
        }
    }

    // 8 + 9. hotels + hotel_access_lines.
    if let Some(name) = repo::hotel_name(&conn, plan_id, dest).await?
        && !name.is_empty()
    {
        view.hotel = Some(HotelSummary { name, access: Vec::new() });
    }
    let access_lines = repo::hotel_access_lines(&conn, plan_id, dest).await?;
    if let Some(h) = view.hotel.as_mut() {
        for line in access_lines {
            if !line.is_empty() {
                h.access.push(line);
            }
        }
    }

    // 10 + 11. plan_offer_selection + plan_offer_includes.
    let mut selected_offer_id = String::new();
    if let Some((id, date, at)) = repo::offer_selection(&conn, plan_id, dest).await?
        && !id.is_empty()
    {
        view.selected_offer = Some(SelectedOffer {
            id: id.clone(),
            date,
            selected_at: at,
        });
        selected_offer_id = id;
    }
    if !selected_offer_id.is_empty() {
        for item in repo::offer_includes(&conn, plan_id, dest, &selected_offer_id).await? {
            if !item.is_empty() {
                view.offer_includes.push(item);
            }
        }
    }

    // 12. days.
    for d in repo::days(&conn, plan_id, dest).await? {
        let weather = if d.weather_label.is_empty() {
            None
        } else {
            Some(DayWeather {
                weather_label: d.weather_label,
                temp_low_c: d.temp_low_c,
                temp_high_c: d.temp_high_c,
                precipitation_pct: d.precipitation_pct,
                weather_code: d.weather_code,
                source_id: d.weather_source_id,
                sourced_at: d.weather_sourced_at,
            })
        };
        view.days.push(DayView {
            day_number: d.day_number,
            date: d.date,
            theme: d.theme,
            day_type: d.day_type,
            weather,
            sessions: HashMap::new(),
        });
    }

    // 13. timesofday (key: day_number -> SessionView).
    for t in repo::timesofday(&conn, plan_id, dest).await? {
        if let Some(d) = view.days.iter_mut().find(|d| d.day_number == t.day_number) {
            d.sessions.insert(
                t.session_type,
                SessionView {
                    time_range_start: t.time_range_start,
                    time_range_end: t.time_range_end,
                    transit_notes: t.transit_notes,
                    focus: t.focus,
                    meals: Vec::new(),
                    activities: Vec::new(),
                },
            );
        }
    }

    // 14. activities (key: day_number, session_type -> activity rows; order
    // by sort_order for stable output when the TS iterates the array).
    for a in repo::activities(&conn, plan_id, dest).await? {
        let act = ActivityView {
            title: a.title,
            booking_required: a.booking_required != 0,
            booking_status: a.booking_status,
            booking_ref: a.booking_ref,
            is_fixed_time: a.is_fixed_time != 0,
            start_time: a.start_time,
            end_time: a.end_time,
            book_by: a.book_by,
            sort_order: a.sort_order,
        };
        if let Some(d) = view.days.iter_mut().find(|d| d.day_number == a.day_number)
            && let Some(s) = d.sessions.get_mut(&a.session_type)
        {
            s.activities.push(act);
        }
    }

    // 15. session_meals (child rows; ordered by sort_order). Grouped into
    // SessionView.meals (Vec<String>) by (day_number, session_type).
    for m in repo::session_meals(&conn, plan_id, dest).await? {
        if m.meal.is_empty() {
            continue;
        }
        if let Some(d) = view.days.iter_mut().find(|d| d.day_number == m.day_number)
            && let Some(s) = d.sessions.get_mut(&m.session_type)
        {
            s.meals.push(m.meal);
        }
    }

    // 16. day_route_segments (per-day ROUTE block). Grouped by day_number.
    for r in repo::day_route_segments(&conn, plan_id, dest).await? {
        let seg = RouteSegment {
            from_place: r.from_place,
            to_place: r.to_place,
            mode: r.mode,
            duration_min: r.duration_min,
            notes: r.notes,
            start_time: r.start_time,
        };
        view.route_segments
            .entry(r.day_number as i32)
            .or_default()
            .push(seg);
    }

    Ok(view)
}

#[cfg(test)]
mod tests {
    use super::*;

    // `--dest` on view commands is parity-only: it must MATCH the plan's active
    // destination (views render only the active destination) and otherwise
    // fail loud — never silently ignored. These lock that contract.

    #[test]
    fn no_dest_always_ok() {
        assert!(assert_dest_matches(None, "okinawa_2026").is_ok());
    }

    #[test]
    fn matching_dest_ok() {
        assert!(assert_dest_matches(Some("okinawa_2026"), "okinawa_2026").is_ok());
    }

    #[test]
    fn mismatching_dest_fails_loud() {
        let err = assert_dest_matches(Some("tokyo_2026"), "okinawa_2026")
            .expect_err("a non-active --dest must be rejected, not ignored");
        // surfaces both the bad value and the active destination so the user
        // can correct the flag.
        assert!(err.contains("tokyo_2026"), "err names the bad --dest: {err}");
        assert!(err.contains("okinawa_2026"), "err names the active dest: {err}");
    }
}
