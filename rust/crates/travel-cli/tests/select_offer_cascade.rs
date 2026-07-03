//! Full real-Turso behavior lock for `select-offer`.
//!
//! This locks the current cascade write surface before moving the inline SQL in
//! `cascade/select_offer.rs` behind `travel-db::repo::*` helpers. It deliberately
//! asserts the current quirks: `selected_date` stays NULL, P3/P4 are populated
//! from the offer, flight/hotel rows are replaced from the package, timeline
//! event order is stable, and the audit back half writes one operation run plus
//! one plan version bump.

use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{bin, db_exec, is_credless, nanos, seed_plan, teardown_plan, Guard};

static SELECT_OFFER_CASCADE_LOCK: Mutex<()> = Mutex::new(());

fn seed_process_statuses(plan: &str, dest: &str) {
    for (process_id, status) in [
        ("process_3_4_packages", "researched"),
        ("process_3_transportation", "pending"),
        ("process_4_accommodation", "pending"),
    ] {
        db_exec(&format!(
            "INSERT OR REPLACE INTO process_statuses \
                (plan_id, destination, process_id, status) \
             VALUES ('{plan}', '{dest}', '{process_id}', '{status}')"
        ))
        .expect("creds");
    }
}

fn seed_offer(plan: &str, dest: &str, offer: &str, selected_date: &str) {
    db_exec(&format!(
        "INSERT OR REPLACE INTO plan_offers \
            (plan_id, destination, id, source_id, type, title, price_per_person, \
             currency, availability, scraped_at, price_total) \
         VALUES ('{plan}', '{dest}', '{offer}', 'zzsource', 'package', \
             'Cascade Lock Package', 61728, 'TWD', 'available', \
             '2026-07-02T00:00:00Z', 123456)"
    ))
    .expect("creds");
    db_exec(&format!(
        "INSERT OR REPLACE INTO plan_offer_date_pricing \
            (plan_id, destination, offer_id, date, price, availability, seats_remaining, currency) \
         VALUES ('{plan}', '{dest}', '{offer}', '{selected_date}', 123456, \
             'available', 6, 'TWD')"
    ))
    .expect("creds");
    for (direction, flight_number, dep_code, dep_time, arr_code, arr_time) in [
        ("outbound", "CI120", "TPE", "08:15", "OKA", "10:45"),
        ("return", "CI121", "OKA", "11:55", "TPE", "12:30"),
    ] {
        db_exec(&format!(
            "INSERT OR REPLACE INTO plan_offer_flights \
                (plan_id, destination, offer_id, direction, flight_number, airline, \
                 airline_code, departure_code, departure_time, arrival_code, arrival_time) \
             VALUES ('{plan}', '{dest}', '{offer}', '{direction}', '{flight_number}', \
                 'China Airlines', 'CI', '{dep_code}', '{dep_time}', '{arr_code}', '{arr_time}')"
        ))
        .expect("creds");
    }
    db_exec(&format!(
        "INSERT OR REPLACE INTO plan_offer_hotels \
            (plan_id, destination, offer_id, name, slug, area, star_rating) \
         VALUES ('{plan}', '{dest}', '{offer}', 'Cascade Test Hotel', NULL, 'naha', 4)"
    ))
    .expect("creds");
    for (sort_order, line) in [
        (0, "Monorail to Makishi"),
        (1, "Walk 6 minutes"),
        (2, "Taxi stand nearby"),
    ] {
        db_exec(&format!(
            "INSERT INTO plan_offer_hotel_access \
                (plan_id, destination, offer_id, sort_order, line) \
             VALUES ('{plan}', '{dest}', '{offer}', {sort_order}, '{line}')"
        ))
        .expect("creds");
    }
}

fn seed_stale_populated_rows(plan: &str, dest: &str) {
    db_exec(&format!(
        "INSERT OR REPLACE INTO flight_legs \
            (plan_id, destination, direction, leg_order, flight_number, airline, \
             airline_code, departure_airport, departure_code, departure_terminal, \
             departure_time, arrival_airport, arrival_code, arrival_terminal, \
             arrival_time, flight_date, populated_from, booked_date) \
         VALUES ('{plan}', '{dest}', 'outbound', 1, 'OLD999', 'Old Air', \
             'OA', 'Old Departure', 'OLD', '1', '00:00', 'Old Arrival', \
             'ZZZ', '2', '01:00', '2026-01-01', 'stale', '2026-01-01')"
    ))
    .expect("creds");
    db_exec(&format!(
        "INSERT OR REPLACE INTO hotels \
            (plan_id, destination, populated_from, name, check_in, notes) \
         VALUES ('{plan}', '{dest}', 'stale', 'Old Hotel', '2026-01-01', 'old notes')"
    ))
    .expect("creds");
    db_exec(&format!(
        "INSERT OR REPLACE INTO hotel_access_lines \
            (plan_id, destination, sort_order, line) \
         VALUES ('{plan}', '{dest}', 99, 'Old access line')"
    ))
    .expect("creds");
}

fn run_select_offer(plan: &str, offer: &str, selected_date: &str) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(["select-offer", offer, selected_date, "--plan-id", plan])
        .output()
        .expect("run select-offer");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[tokio::test]
async fn select_offer_writes_full_package_cascade_surface() {
    let _guard = SELECT_OFFER_CASCADE_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    if db_exec("SELECT 1").is_none() {
        eprintln!("skipping select-offer cascade test (no Turso creds)");
        return;
    }

    let tag = nanos();
    let plan = format!("zztest{tag}");
    let dest = format!("zztest{tag}");
    let offer = format!("zzoffer{tag}");
    let selected_date = "2026-09-04";

    teardown_plan(&plan, &dest);
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });

    seed_plan(&plan, &dest, 7);
    seed_process_statuses(&plan, &dest);
    seed_offer(&plan, &dest, &offer, selected_date);
    seed_stale_populated_rows(&plan, &dest);

    let (ok, out, err) = run_select_offer(&plan, &offer, selected_date);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds mid-test): {}", err.trim());
        return;
    }
    assert!(ok, "select-offer should succeed; err={err}\nout={out}");

    let selection = db_exec(&format!(
        "SELECT selected_offer_id || '|' || (selected_date IS NULL) || '|' || \
                (selected_at IS NOT NULL) || '|' || (updated_at IS NOT NULL) AS row_text \
         FROM plan_offer_selection \
         WHERE plan_id = '{plan}' AND destination = '{dest}'"
    ))
    .expect("creds");
    assert_eq!(
        selection.scalar().as_deref(),
        Some(format!("{offer}|1|1|1").as_str()),
        "plan_offer_selection should point at the offer and keep selected_date NULL; out={selection}"
    );
    let selection_count = db_exec(&format!(
        "SELECT COUNT(*) AS value_text FROM plan_offer_selection \
         WHERE plan_id = '{plan}' AND destination = '{dest}'"
    ))
    .expect("creds");
    assert_eq!(selection_count.scalar().as_deref(), Some("1"));

    let statuses = db_exec(&format!(
        "SELECT process_id || '=' || status AS row_text \
         FROM process_statuses \
         WHERE plan_id = '{plan}' AND destination = '{dest}' \
           AND process_id IN ('process_3_4_packages', 'process_3_transportation', \
                              'process_4_accommodation') \
         ORDER BY CASE process_id \
             WHEN 'process_3_4_packages' THEN 0 \
             WHEN 'process_3_transportation' THEN 1 \
             ELSE 2 END"
    ))
    .expect("creds");
    assert_eq!(
        statuses.column(),
        vec![
            "process_3_4_packages=selected".to_string(),
            "process_3_transportation=populated".to_string(),
            "process_4_accommodation=populated".to_string(),
        ],
        "process_statuses transitions should match the package cascade; out={statuses}"
    );

    let flight_count = db_exec(&format!(
        "SELECT COUNT(*) AS value_text FROM flight_legs \
         WHERE plan_id = '{plan}' AND destination = '{dest}'"
    ))
    .expect("creds");
    assert_eq!(flight_count.scalar().as_deref(), Some("2"), "flight legs are delete-reinserted");
    let stale_flights = db_exec(&format!(
        "SELECT COUNT(*) AS value_text FROM flight_legs \
         WHERE plan_id = '{plan}' AND destination = '{dest}' \
           AND (leg_order <> 0 OR flight_number = 'OLD999' OR populated_from = 'stale')"
    ))
    .expect("creds");
    assert_eq!(stale_flights.scalar().as_deref(), Some("0"), "stale flight rows must be deleted");
    let flights = db_exec(&format!(
        "SELECT direction || '|' || leg_order || '|' || COALESCE(flight_number, '<NULL>') || \
                '|' || COALESCE(airline, '<NULL>') || '|' || COALESCE(airline_code, '<NULL>') || \
                '|' || (departure_airport IS NULL) || '|' || COALESCE(departure_code, '<NULL>') || \
                '|' || (departure_terminal IS NULL) || '|' || COALESCE(departure_time, '<NULL>') || \
                '|' || (arrival_airport IS NULL) || '|' || COALESCE(arrival_code, '<NULL>') || \
                '|' || (arrival_terminal IS NULL) || '|' || COALESCE(arrival_time, '<NULL>') || \
                '|' || (flight_date IS NULL) || '|' || COALESCE(populated_from, '<NULL>') || \
                '|' || COALESCE(booked_date, '<NULL>') || '|' || (updated_at IS NOT NULL) AS row_text \
         FROM flight_legs \
         WHERE plan_id = '{plan}' AND destination = '{dest}' \
         ORDER BY CASE direction WHEN 'outbound' THEN 0 ELSE 1 END"
    ))
    .expect("creds");
    assert_eq!(
        flights.column(),
        vec![
            format!("outbound|0|CI120|China Airlines|CI|1|TPE|1|08:15|1|OKA|1|10:45|1|package:{offer}|{selected_date}|1"),
            format!("return|0|CI121|China Airlines|CI|1|OKA|1|11:55|1|TPE|1|12:30|1|package:{offer}|{selected_date}|1"),
        ],
        "flight_legs should match select_offer.rs fixed INSERT OR REPLACE shape; out={flights}"
    );

    let hotel_count = db_exec(&format!(
        "SELECT COUNT(*) AS value_text FROM hotels \
         WHERE plan_id = '{plan}' AND destination = '{dest}'"
    ))
    .expect("creds");
    assert_eq!(hotel_count.scalar().as_deref(), Some("1"));
    let hotel = db_exec(&format!(
        "SELECT COALESCE(name, '<NULL>') || '|' || COALESCE(check_in, '<NULL>') || \
                '|' || (notes IS NULL) || '|' || COALESCE(populated_from, '<NULL>') || \
                '|' || (updated_at IS NOT NULL) AS row_text \
         FROM hotels \
         WHERE plan_id = '{plan}' AND destination = '{dest}'"
    ))
    .expect("creds");
    assert_eq!(
        hotel.scalar().as_deref(),
        Some(format!("Cascade Test Hotel|{selected_date}|1|package:{offer}|1").as_str()),
        "hotels row should be replaced from the offer; out={hotel}"
    );

    let access = db_exec(&format!(
        "SELECT sort_order || '|' || line || '|' || (updated_at IS NOT NULL) AS row_text \
         FROM hotel_access_lines \
         WHERE plan_id = '{plan}' AND destination = '{dest}' \
         ORDER BY sort_order"
    ))
    .expect("creds");
    assert_eq!(
        access.column(),
        vec![
            "0|Monorail to Makishi|1".to_string(),
            "1|Walk 6 minutes|1".to_string(),
            "2|Taxi stand nearby|1".to_string(),
        ],
        "hotel_access_lines should be delete-reinserted in offer sort_order; out={access}"
    );

    let timeline = db_exec(&format!(
        "SELECT sort_order || '|' || process_id || '|' || event || '|' || \
                COALESCE(from_state, '<NULL>') || '>' || COALESCE(to_state, '<NULL>') AS row_text \
         FROM plan_events \
         WHERE plan_id = '{plan}' AND scope = 'timeline' \
         ORDER BY sort_order"
    ))
    .expect("creds");
    assert_eq!(
        timeline.column(),
        vec![
            "0|process_3_4_packages|status_changed|researched>selected".to_string(),
            "1|process_3_4_packages|offer_selected|<NULL>><NULL>".to_string(),
            "2|process_3_transportation|status_changed|pending>populated".to_string(),
            "3|process_4_accommodation|status_changed|pending>populated".to_string(),
            "4||cascade_populated|<NULL>><NULL>".to_string(),
        ],
        "timeline event order should match select_offer.rs EVENT ORDER; out={timeline}"
    );

    let dest_process_events = db_exec(&format!(
        "SELECT process_id || '|' || sort_order || '|' || event || '|' || \
                COALESCE(from_state, '<NULL>') || '>' || COALESCE(to_state, '<NULL>') AS row_text \
         FROM plan_events \
         WHERE plan_id = '{plan}' AND scope = 'dest_process' AND destination = '{dest}' \
         ORDER BY CASE process_id \
             WHEN 'process_3_4_packages' THEN 0 \
             WHEN 'process_3_transportation' THEN 1 \
             ELSE 2 END, sort_order"
    ))
    .expect("creds");
    assert_eq!(
        dest_process_events.column(),
        vec![
            "process_3_4_packages|0|status_changed|researched>selected".to_string(),
            "process_3_4_packages|1|offer_selected|<NULL>><NULL>".to_string(),
            "process_3_transportation|0|status_changed|pending>populated".to_string(),
            "process_4_accommodation|0|status_changed|pending>populated".to_string(),
        ],
        "dest_process events should be emitted for status changes and offer_selected only; out={dest_process_events}"
    );

    let offer_selected_kv = db_exec(&format!(
        "SELECT key || '=' || COALESCE(value, '<NULL>') AS row_text \
         FROM plan_event_data \
         WHERE plan_id = '{plan}' AND scope = 'timeline' AND destination = '' \
           AND process_id = 'process_3_4_packages' AND sort_order = 1 \
         ORDER BY key"
    ))
    .expect("creds");
    assert_eq!(
        offer_selected_kv.column(),
        vec![
            format!("date={selected_date}"),
            "hotel=Cascade Test Hotel".to_string(),
            format!("offer_id={offer}"),
            "offer_name=undefined".to_string(),
            "price_total=123456".to_string(),
        ],
        "offer_selected timeline KV should include the current five keys; out={offer_selected_kv}"
    );
    let cascade_kv = db_exec(&format!(
        "SELECT key || '=' || COALESCE(value, '<NULL>') AS row_text \
         FROM plan_event_data \
         WHERE plan_id = '{plan}' AND scope = 'timeline' AND destination = '' \
           AND process_id = '' AND sort_order = 4 \
         ORDER BY key"
    ))
    .expect("creds");
    assert_eq!(
        cascade_kv.column(),
        vec![
            "populated=process_3_transportation, process_4_accommodation".to_string(),
            format!("source=package:{offer}"),
        ],
        "cascade_populated timeline KV should identify populated processes and source; out={cascade_kv}"
    );

    let operation_count = db_exec(&format!(
        "SELECT COUNT(*) AS value_text FROM operation_runs WHERE plan_id = '{plan}'"
    ))
    .expect("creds");
    assert_eq!(operation_count.scalar().as_deref(), Some("1"), "one operation run total");
    let operation = db_exec(&format!(
        "SELECT command_type || '|' || COALESCE(command_summary, '<NULL>') || '|' || \
                version_before || '>' || COALESCE(version_after, -1) || '|' || status AS row_text \
         FROM operation_runs WHERE plan_id = '{plan}'"
    ))
    .expect("creds");
    assert_eq!(
        operation.scalar().as_deref(),
        Some(format!("select-offer|{offer}|7>8|completed").as_str()),
        "operation_runs should record one select-offer audit row; out={operation}"
    );
    let version = db_exec(&format!("SELECT version AS value_text FROM plans WHERE plan_id = '{plan}'"))
        .expect("creds");
    assert_eq!(version.scalar().as_deref(), Some("8"), "plans.version should bump by one");
}