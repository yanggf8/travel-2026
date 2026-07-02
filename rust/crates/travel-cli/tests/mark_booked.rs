//! Real-Turso behavior-LOCK integration test for `mark-booked`.
//!
//! Locks the CURRENT (un-migrated) DB write surface of `mark_booked.rs` BEFORE its
//! DAL migration, so the migration can be proven byte-for-byte behavior-preserving.
//!
//! Seeds a zztest plan + plan_metadata + the P3+4 process ladder (as `selected`) +
//! event_log_state + one plan_destinations row + a plan_offer_selection row + a
//! PENDING bookings_current row keyed exactly as the re-sync will re-key it. Runs
//! `mark-booked --dest <slug>` and asserts the full side-effect set:
//!   - process_statuses P3+4 selected -> booked
//!   - event_log_dest_processes P3+4 -> booked (upserted in lockstep)
//!   - cascade_dirty_flags P3+4 dirty -> 0
//!   - event_log_state.current_focus -> '<dest>.process_5_daily_itinerary'
//!   - event_log_next_actions replaced with the 3 canonical follow-up actions
//!   - exactly one booking_confirmed timeline plan_events row (+ 3 KV data rows)
//!   - bookings_current package row flips pending -> booked (via the save() re-sync)
//!     + bookings_current_payload re-materialized
//!   - exactly one operation_runs row command_type='mark-booked'
//!   - plans.version bumps by exactly one
//!
//! Scoped by a unique zztest plan/dest/booking key; NEVER touches a real plan.
//! Skips cleanly if Turso creds are absent.

use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

mod common;
use common::Guard;

static MARK_BOOKED_LOCK: Mutex<()> = Mutex::new(());

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_travel")
}

fn nanos() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
}

fn db_exec(sql: &str) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(["db", "exec", sql])
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .expect("run db exec");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn is_skip(stderr: &str) -> bool {
    stderr.contains("turso auth login")
        || stderr.contains("Missing Turso")
        || stderr.contains("Missing Turso data")
        || stderr.contains("failed to connect to Turso")
        || stderr.contains("TRAVEL_TURSO")
}

fn db_or_skip(sql: &str) -> Option<String> {
    let (ok, stdout, stderr) = db_exec(sql);
    if ok {
        return Some(stdout);
    }
    if is_skip(&stderr) {
        eprintln!("skipping mark-booked test (no Turso creds): {}", stderr.trim());
        return None;
    }
    panic!("travel db exec failed: {}\nSQL: {sql}", stderr.trim());
}

fn scalar(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()))
}

fn column(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()))
        .filter(|v| !v.is_empty())
        .collect()
}

/// Seed the minimal state `mark-booked` needs to book P3+4 and re-sync one
/// package booking. `booking_key` must equal the re-sync's derived key
/// (`<trip_id>:<dest>:package:<offer_id>`) so the pending row is replaced.
fn seed(plan: &str, dest: &str, offer_id: &str, booking_key: &str) -> bool {
    // NOTE: no `--` comments and no stray `;`/`'` inside this SQL — `db exec` splits
    // on `;` BEFORE stripping comment lines, so an inline comment would corrupt the
    // following statements (see the seed-splitter rule in CLAUDE.md).
    let trip_id = plan.replace('-', "_");
    let sql = format!(
        "INSERT OR REPLACE INTO plans (plan_id, schema_version, version) \
           VALUES ('{plan}', '4.2.0', 0); \
         INSERT OR REPLACE INTO plan_metadata (plan_id, schema_version, active_destination) \
           VALUES ('{plan}', '4.2.0', '{dest}'); \
         INSERT OR REPLACE INTO process_statuses (plan_id, destination, process_id, status) \
           VALUES ('{plan}', '{dest}', 'process_3_4_packages', 'selected'); \
         INSERT OR REPLACE INTO event_log_dest_processes (plan_id, destination, process_id, status) \
           VALUES ('{plan}', '{dest}', 'process_3_4_packages', 'selected'); \
         INSERT OR REPLACE INTO cascade_dirty_flags (plan_id, destination, process_id, dirty) \
           VALUES ('{plan}', '{dest}', 'process_3_4_packages', 1); \
         INSERT OR REPLACE INTO event_log_state \
           (plan_id, session, project, version, current_focus, active_destination) \
           VALUES ('{plan}', 'test-session', 'travel-2026', '4.2.0', \
                   '{dest}.process_2_destination', '{dest}'); \
         INSERT OR REPLACE INTO event_log_next_actions (plan_id, sort_order, action) \
           VALUES ('{plan}', 0, 'stale_action_must_be_replaced'); \
         INSERT OR REPLACE INTO plan_destinations (plan_id, slug, display_name, status) \
           VALUES ('{plan}', '{dest}', 'ZZ Test Dest', 'active'); \
         INSERT OR REPLACE INTO plan_offer_selection \
           (plan_id, destination, selected_offer_id, selected_date) \
           VALUES ('{plan}', '{dest}', '{offer_id}', '2026-09-04'); \
         INSERT OR REPLACE INTO bookings_current \
           (booking_key, trip_id, destination, category, subtype, title, status, \
            price_currency, origin_path) \
           VALUES ('{booking_key}', '{trip_id}', '{dest}', 'package', 'package', \
                   'package - {offer_id}', 'pending', 'TWD', \
                   'destinations.{dest}.process_3_4_packages');"
    );
    db_or_skip(&sql).is_some()
}

fn teardown(plan: &str, dest: &str, booking_key: &str) {
    let trip_id = plan.replace('-', "_");
    let sql = format!(
        "DELETE FROM bookings_current_payload WHERE booking_key = '{booking_key}'; \
         DELETE FROM bookings_current WHERE trip_id = '{trip_id}'; \
         DELETE FROM plan_offer_selection WHERE plan_id = '{plan}' AND destination = '{dest}'; \
         DELETE FROM plan_destinations WHERE plan_id = '{plan}'; \
         DELETE FROM cascade_dirty_flags WHERE plan_id = '{plan}'; \
         DELETE FROM event_log_dest_processes WHERE plan_id = '{plan}'; \
         DELETE FROM event_log_next_actions WHERE plan_id = '{plan}'; \
         DELETE FROM event_log_state WHERE plan_id = '{plan}'; \
         DELETE FROM process_statuses WHERE plan_id = '{plan}'; \
         DELETE FROM plan_event_data WHERE plan_id = '{plan}'; \
         DELETE FROM plan_events WHERE plan_id = '{plan}'; \
         DELETE FROM operation_runs WHERE plan_id = '{plan}'; \
         DELETE FROM plan_metadata WHERE plan_id = '{plan}'; \
         DELETE FROM plans WHERE plan_id = '{plan}';"
    );
    let _ = db_exec(&sql);
}

fn run_mark_booked(plan: &str, dest: &str) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(["mark-booked", "--dest", dest])
        .env("TRAVEL_PLAN_ID", plan)
        .output()
        .expect("run mark-booked");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn mark_booked_locks_full_write_surface() {
    let _lock = MARK_BOOKED_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    if db_or_skip("SELECT 1 AS n").is_none() {
        return;
    }

    let tag = nanos();
    let plan = format!("zztest{tag}");
    let dest = format!("zztest_dest_{tag}");
    let offer_id = format!("zztest_offer_{tag}");
    let trip_id = plan.replace('-', "_");
    let booking_key = format!("{trip_id}:{dest}:package:{offer_id}");

    // Defensive pre-clean, then arm the panic-safe teardown guard immediately.
    teardown(&plan, &dest, &booking_key);
    let _g = Guard::new({
        let plan = plan.clone();
        let dest = dest.clone();
        let booking_key = booking_key.clone();
        move || teardown(&plan, &dest, &booking_key)
    });

    assert!(seed(&plan, &dest, &offer_id, &booking_key), "seed plan");

    let (ok, stdout, stderr) = run_mark_booked(&plan, &dest);
    if !ok && is_skip(&stderr) {
        eprintln!("skipping mark-booked test (no Turso creds): {}", stderr.trim());
        return;
    }
    assert!(ok, "mark-booked should succeed; stdout={stdout} stderr={stderr}");
    assert!(
        stdout.contains(&format!("Marking booking as confirmed for {dest}:")),
        "stdout should include the header; stdout={stdout}"
    );
    assert!(
        stdout.contains("P3+4 Packages: selected → booking → booked"),
        "stdout should report the P3+4 transition; stdout={stdout}"
    );
    assert!(
        stdout.contains("Booking marked as confirmed"),
        "stdout should include the completion line; stdout={stdout}"
    );

    // process_statuses: P3+4 selected -> booked.
    let ps = db_or_skip(&format!(
        "SELECT status AS v FROM process_statuses \
         WHERE plan_id = '{plan}' AND destination = '{dest}' \
           AND process_id = 'process_3_4_packages'"
    ))
    .unwrap();
    assert_eq!(
        scalar(&ps).as_deref(),
        Some("booked"),
        "P3+4 process_statuses should move selected -> booked; out={ps}"
    );

    // event_log_dest_processes: mirrored to booked in lockstep.
    let elp = db_or_skip(&format!(
        "SELECT status AS v FROM event_log_dest_processes \
         WHERE plan_id = '{plan}' AND destination = '{dest}' \
           AND process_id = 'process_3_4_packages'"
    ))
    .unwrap();
    assert_eq!(
        scalar(&elp).as_deref(),
        Some("booked"),
        "event_log_dest_processes P3+4 should be booked; out={elp}"
    );

    // cascade_dirty_flags: dirty cleared to 0.
    let dirty = db_or_skip(&format!(
        "SELECT dirty AS v FROM cascade_dirty_flags \
         WHERE plan_id = '{plan}' AND destination = '{dest}' \
           AND process_id = 'process_3_4_packages'"
    ))
    .unwrap();
    assert_eq!(
        scalar(&dirty).as_deref(),
        Some("0"),
        "cascade_dirty_flags P3+4 dirty should be cleared to 0; out={dirty}"
    );

    // event_log_state.current_focus repointed at process_5.
    let focus = db_or_skip(&format!(
        "SELECT current_focus AS v FROM event_log_state WHERE plan_id = '{plan}'"
    ))
    .unwrap();
    assert_eq!(
        scalar(&focus).as_deref(),
        Some(format!("{dest}.process_5_daily_itinerary").as_str()),
        "event_log_state.current_focus should point at process_5; out={focus}"
    );

    // event_log_next_actions replaced with the 3 canonical actions, in order.
    let actions = db_or_skip(&format!(
        "SELECT sort_order || ':' || action AS li FROM event_log_next_actions \
         WHERE plan_id = '{plan}' ORDER BY sort_order"
    ))
    .unwrap();
    assert_eq!(
        column(&actions),
        vec![
            "0:plan_daily_itinerary",
            "1:book_teamlab_tickets",
            "2:research_restaurant_reservations",
        ],
        "next actions should be fully replaced with the 3 canonical follow-ups; out={actions}"
    );

    // exactly one booking_confirmed timeline plan_events row.
    let ev = db_or_skip(&format!(
        "SELECT COUNT(*) AS n FROM plan_events \
         WHERE plan_id = '{plan}' AND scope = 'timeline' AND event = 'booking_confirmed'"
    ))
    .unwrap();
    assert_eq!(
        scalar(&ev).as_deref(),
        Some("1"),
        "exactly one booking_confirmed timeline event should be written; out={ev}"
    );

    // its 3 KV rows: destination, processes, confirmed_at.
    let ev_kv = db_or_skip(&format!(
        "SELECT key AS li FROM plan_event_data d \
         WHERE d.plan_id = '{plan}' AND d.scope = 'timeline' \
           AND d.sort_order = ( \
             SELECT sort_order FROM plan_events \
             WHERE plan_id = '{plan}' AND scope = 'timeline' AND event = 'booking_confirmed') \
         ORDER BY d.key"
    ))
    .unwrap();
    assert_eq!(
        column(&ev_kv),
        vec!["confirmed_at", "destination", "processes"],
        "booking_confirmed event should carry its 3 KV keys; out={ev_kv}"
    );
    let ev_dest = db_or_skip(&format!(
        "SELECT value AS v FROM plan_event_data \
         WHERE plan_id = '{plan}' AND scope = 'timeline' AND key = 'destination' \
           AND sort_order = ( \
             SELECT sort_order FROM plan_events \
             WHERE plan_id = '{plan}' AND scope = 'timeline' AND event = 'booking_confirmed')"
    ))
    .unwrap();
    assert_eq!(
        scalar(&ev_dest).as_deref(),
        Some(dest.as_str()),
        "booking_confirmed destination KV should equal the booked dest; out={ev_dest}"
    );

    // bookings_current: pending -> booked via the save() re-sync.
    let bc = db_or_skip(&format!(
        "SELECT status AS v FROM bookings_current WHERE booking_key = '{booking_key}'"
    ))
    .unwrap();
    assert_eq!(
        scalar(&bc).as_deref(),
        Some("booked"),
        "bookings_current package row should flip pending -> booked; out={bc}"
    );

    // exactly one such package row for this trip (re-sync deletes then re-inserts).
    let bc_count = db_or_skip(&format!(
        "SELECT COUNT(*) AS n FROM bookings_current WHERE trip_id = '{trip_id}'"
    ))
    .unwrap();
    assert_eq!(
        scalar(&bc_count).as_deref(),
        Some("1"),
        "re-sync should leave exactly one bookings_current row for the trip; out={bc_count}"
    );

    // exactly one mark-booked operation_run.
    let op = db_or_skip(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs \
         WHERE plan_id = '{plan}' AND command_type = 'mark-booked'"
    ))
    .unwrap();
    assert_eq!(
        scalar(&op).as_deref(),
        Some("1"),
        "exactly one mark-booked operation_run should be written; out={op}"
    );

    // plans.version bumped by exactly one (0 -> 1).
    let ver = db_or_skip(&format!("SELECT version AS v FROM plans WHERE plan_id = '{plan}'"))
        .unwrap();
    assert_eq!(
        scalar(&ver).as_deref(),
        Some("1"),
        "plans.version should bump by one; out={ver}"
    );

    // Guard's Drop runs teardown here (and on any panic above).
}
