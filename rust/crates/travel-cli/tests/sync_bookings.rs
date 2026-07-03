use std::process::Command;
use std::thread;
use std::time::Duration;

mod common;
use common::{bin, db_exec, db_exec_teardown, is_credless, nanos, teardown_plan, Guard};

const ACTIVITY_TITLE: &str = "Reservation Lock Activity";
const AREA: &str = "Harbor Deck";
const BOOKING_URL: &str = "https://example.test/sync-bookings";
const BOOK_BY: &str = "2026-06-15";

fn cleanup(plan_id: &str, trip_id: &str, booking_key: &str, panic_on_failure: bool) -> bool {
    if panic_on_failure && db_exec("SELECT 1").is_none() {
        return false;
    }

    let booking_sql = format!(
        "DELETE FROM bookings_event_data WHERE booking_key = '{booking_key}'; \
         DELETE FROM bookings_events WHERE booking_key = '{booking_key}'; \
         DELETE FROM bookings_current_payload WHERE booking_key = '{booking_key}'; \
         DELETE FROM bookings_current WHERE trip_id = '{trip_id}' OR booking_key = '{booking_key}'; \
         DELETE FROM activity_tags WHERE activity_id IN (SELECT id FROM activities WHERE plan_id = '{plan_id}');"
    );

    if panic_on_failure {
        db_exec(&booking_sql).expect("cleanup booking rows");
        teardown_plan(plan_id, "");
        true
    } else {
        let _ = db_exec_teardown(&booking_sql);
        teardown_plan(plan_id, "");
        true
    }
}

fn seed_plan_with_one_activity_booking(plan_id: &str, dest: &str, activity_id: &str) -> bool {
    let sql = format!(
        "INSERT INTO plans (plan_id, schema_version, version) \
           VALUES ('{plan_id}', '4.2.0', 0); \
         INSERT INTO plan_metadata (plan_id, schema_version, active_destination) \
           VALUES ('{plan_id}', '4.2.0', '{dest}'); \
         INSERT INTO plan_destinations (plan_id, slug, display_name, status, created_at, updated_at) \
           VALUES ('{plan_id}', '{dest}', 'Sync Bookings Test', 'draft', datetime('now'), datetime('now')); \
         INSERT INTO days (plan_id, destination, day_number, date, theme, day_type, status) \
           VALUES ('{plan_id}', '{dest}', 1, '2026-06-20', 'Booking lock day', 'full', 'draft'); \
         INSERT INTO activities \
           (id, plan_id, destination, day_number, session_type, sort_order, title, area, \
            duration_min, booking_required, booking_url, booking_status, booking_ref, book_by, \
            cost_estimate, is_fixed_time, priority) \
         VALUES \
           ('{activity_id}', '{plan_id}', '{dest}', 1, 'morning', 0, '{ACTIVITY_TITLE}', '{AREA}', \
            90, 1, '{BOOKING_URL}', 'pending', 'SYNC-REF-1', '{BOOK_BY}', \
            1200, 0, 'want');"
    );

    db_exec(&sql).is_some()
}

fn update_seeded_booking(activity_id: &str) -> bool {
    db_exec(&format!(
        "UPDATE activities \
         SET booking_status = 'booked', booking_ref = 'SYNC-REF-2', cost_estimate = 1500 \
         WHERE id = '{activity_id}';"
    ))
    .is_some()
}

fn run_sync(plan_id: &str, trip_id: &str) -> Option<String> {
    let out = Command::new(bin())
        .args(["sync-bookings", "--plan-id", plan_id, "--trip-id", trip_id])
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .expect("run travel sync-bookings");

    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        return Some(stdout);
    }

    let stderr = String::from_utf8_lossy(&out.stderr);
    if is_credless(&stderr) {
        eprintln!("skipping sync-bookings Turso test: {}", stderr.trim());
        return None;
    }

    panic!(
        "travel sync-bookings failed: {}\nstdout: {}",
        stderr.trim(),
        stdout.trim()
    );
}

fn count(sql: &str) -> Option<i64> {
    let n = db_exec(sql)?
        .scalar()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    Some(n)
}

fn values(sql: &str, _prefix: &str) -> Option<Vec<String>> {
    Some(db_exec(sql)?.column())
}

fn assert_created_rows(trip_id: &str, dest: &str, booking_key: &str) {
    assert_eq!(
        count(&format!(
            "SELECT COUNT(*) AS n FROM bookings_current WHERE trip_id = '{trip_id}'"
        )),
        Some(1)
    );

    let current = values(
        &format!(
            "SELECT booking_key || '|' || trip_id || '|' || destination || '|' || category || '|' || \
                    COALESCE(subtype, '') || '|' || title || '|' || status || '|' || \
                    COALESCE(reference, '') || '|' || COALESCE(book_by, '') || '|' || \
                    COALESCE(price_amount, -1) || '|' || price_currency || '|' || COALESCE(origin_path, '') AS rowval \
             FROM bookings_current \
             WHERE trip_id = '{trip_id}' \
             ORDER BY booking_key"
        ),
        "rowval: ",
    )
    .expect("query bookings_current");
    assert_eq!(
        current,
        vec![format!(
            "{booking_key}|{trip_id}|{dest}|activity|day1_morning|{ACTIVITY_TITLE}|pending|SYNC-REF-1|{BOOK_BY}|1200|JPY|destinations.{dest}.process_5_daily_itinerary.days[0].morning"
        )]
    );

    let payload = values(
        &format!(
            "SELECT sort_order || ':' || key || '=' || value AS kv \
             FROM bookings_current_payload \
             WHERE booking_key = '{booking_key}' \
             ORDER BY sort_order"
        ),
        "kv: ",
    )
    .expect("query bookings_current_payload");
    assert_eq!(
        payload,
        vec![
            format!("0:area={AREA}"),
            format!("1:booking_url={BOOKING_URL}"),
        ]
    );

    assert_eq!(
        count(&format!(
            "SELECT COUNT(*) AS n FROM bookings_events WHERE booking_key = '{booking_key}'"
        )),
        Some(1)
    );

    let events = values(
        &format!(
            "SELECT event_type || '|' || COALESCE(new_status, '') || '|' || \
                    COALESCE(reference, '') || '|' || COALESCE(book_by, '') || '|' || \
                    COALESCE(amount, -1) || '|' || COALESCE(currency, '') AS rowval \
             FROM bookings_events \
             WHERE booking_key = '{booking_key}' \
             ORDER BY id"
        ),
        "rowval: ",
    )
    .expect("query bookings_events");
    assert_eq!(
        events,
        vec![format!("created|pending|SYNC-REF-1|{BOOK_BY}|1200|JPY")]
    );

    let event_data = values(
        &format!(
            "SELECT sort_order || ':' || key || '=' || value AS kv \
             FROM bookings_event_data \
             WHERE booking_key = '{booking_key}' \
             ORDER BY event_at, sort_order"
        ),
        "kv: ",
    )
    .expect("query bookings_event_data");
    assert_eq!(
        event_data,
        vec![
            format!("0:area={AREA}"),
            format!("1:booking_url={BOOKING_URL}"),
        ]
    );
}

fn assert_updated_rows(trip_id: &str, dest: &str, booking_key: &str) {
    assert_eq!(
        count(&format!(
            "SELECT COUNT(*) AS n FROM bookings_current WHERE trip_id = '{trip_id}'"
        )),
        Some(1)
    );

    let current = values(
        &format!(
            "SELECT booking_key || '|' || trip_id || '|' || destination || '|' || category || '|' || \
                    COALESCE(subtype, '') || '|' || title || '|' || status || '|' || \
                    COALESCE(reference, '') || '|' || COALESCE(book_by, '') || '|' || \
                    COALESCE(price_amount, -1) || '|' || price_currency || '|' || COALESCE(origin_path, '') AS rowval \
             FROM bookings_current \
             WHERE trip_id = '{trip_id}' \
             ORDER BY booking_key"
        ),
        "rowval: ",
    )
    .expect("query bookings_current after update");
    assert_eq!(
        current,
        vec![format!(
            "{booking_key}|{trip_id}|{dest}|activity|day1_morning|{ACTIVITY_TITLE}|booked|SYNC-REF-2|{BOOK_BY}|1500|JPY|destinations.{dest}.process_5_daily_itinerary.days[0].morning"
        )]
    );

    let payload = values(
        &format!(
            "SELECT sort_order || ':' || key || '=' || value AS kv \
             FROM bookings_current_payload \
             WHERE booking_key = '{booking_key}' \
             ORDER BY sort_order"
        ),
        "kv: ",
    )
    .expect("query bookings_current_payload after update");
    assert_eq!(
        payload,
        vec![
            format!("0:area={AREA}"),
            format!("1:booking_url={BOOKING_URL}"),
        ]
    );

    assert_eq!(
        count(&format!(
            "SELECT COUNT(*) AS n FROM bookings_events WHERE booking_key = '{booking_key}'"
        )),
        Some(2)
    );

    let events = values(
        &format!(
            "SELECT event_type || '|' || COALESCE(new_status, '') || '|' || \
                    COALESCE(reference, '') || '|' || COALESCE(book_by, '') || '|' || \
                    COALESCE(amount, -1) || '|' || COALESCE(currency, '') AS rowval \
             FROM bookings_events \
             WHERE booking_key = '{booking_key}' \
             ORDER BY id"
        ),
        "rowval: ",
    )
    .expect("query bookings_events after update");
    assert_eq!(
        events,
        vec![
            format!("created|pending|SYNC-REF-1|{BOOK_BY}|1200|JPY"),
            format!("updated|booked|SYNC-REF-2|{BOOK_BY}|1500|JPY"),
        ]
    );

    let event_data = values(
        &format!(
            "SELECT e.event_type || ':' || d.sort_order || ':' || d.key || '=' || d.value AS kv \
             FROM bookings_events e \
             JOIN bookings_event_data d \
               ON d.booking_key = e.booking_key AND d.event_at = e.event_at \
             WHERE e.booking_key = '{booking_key}' \
             ORDER BY e.id, d.sort_order"
        ),
        "kv: ",
    )
    .expect("query bookings_event_data after update");
    assert_eq!(
        event_data,
        vec![
            format!("created:0:area={AREA}"),
            format!("created:1:booking_url={BOOKING_URL}"),
            format!("updated:0:area={AREA}"),
            format!("updated:1:booking_url={BOOKING_URL}"),
        ]
    );
}

#[test]
fn sync_bookings_records_created_and_updated_events() {
    let tag = nanos();
    let plan_id = format!("zztest-sync-bookings-{tag}");
    let dest = format!("zzsync_{tag}");
    let trip_id = format!("zzsync_trip_{tag}");
    let activity_id = format!("act-sync-bookings-{tag}");
    let booking_key = format!("{trip_id}:{dest}:activity:1:morning:{activity_id}");

    if !cleanup(&plan_id, &trip_id, &booking_key, true) {
        return;
    }

    let _guard = Guard::new({
        let plan_id = plan_id.clone();
        let trip_id = trip_id.clone();
        let booking_key = booking_key.clone();
        move || {
            let _ = cleanup(&plan_id, &trip_id, &booking_key, false);
        }
    });

    if !seed_plan_with_one_activity_booking(&plan_id, &dest, &activity_id) {
        return;
    }

    let stdout = match run_sync(&plan_id, &trip_id) {
        Some(stdout) => stdout,
        None => return,
    };
    assert!(
        stdout.contains("Synced 1 bookings to Turso."),
        "created sync stdout should report one booking; got {stdout}"
    );
    assert_created_rows(&trip_id, &dest, &booking_key);

    thread::sleep(Duration::from_millis(1100));

    if !update_seeded_booking(&activity_id) {
        return;
    }

    let stdout = match run_sync(&plan_id, &trip_id) {
        Some(stdout) => stdout,
        None => return,
    };
    assert!(
        stdout.contains("Synced 1 bookings to Turso."),
        "updated sync stdout should report one booking; got {stdout}"
    );
    assert_updated_rows(&trip_id, &dest, &booking_key);
}