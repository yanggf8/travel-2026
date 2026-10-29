//! `validate data` must surface booking drift between the plan's normalized
//! tables and `bookings_current` ([bookings] warnings) — the gap that let an
//! activity sit at activities.booking_status='booked' while its
//! bookings_current row said 'pending' for a month with every validator green.
//!
//! Contract (design-reviewed): drift and missing derived rows are WARNINGS
//! (historical unsynced plans must not block the pre-commit hook); a failed
//! check (connection/query error) is an ERROR, never a silent skip; rows the
//! plan cannot derive — accommodation, written directly by set-accommodation —
//! are NOT db-only drift and must not be reported.
//!
//! Real-Turso integration test; scoped by unique zztest fixtures; skips
//! cleanly if creds are absent. Serialized per file via a static mutex.

mod common;
use common::{bin, db_exec, nanos, seed_plan, teardown_plan, Guard};

use std::process::Command;
use std::sync::Mutex;

static VBI_LOCK: Mutex<()> = Mutex::new(());

fn run_cmd(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(args)
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Seed one bookable activity (booking_status given by caller) on day 1 morning.
fn seed_activity(plan: &str, dest: &str, id: &str, booking_status: &str) {
    db_exec(&format!(
        "INSERT OR REPLACE INTO activities \
           (id, plan_id, destination, day_number, session_type, sort_order, title, \
            booking_required, booking_status, is_fixed_time, priority, source) \
         VALUES ('{id}', '{plan}', '{dest}', 1, 'morning', 0, 'ZZ Test Activity', \
                 1, '{booking_status}', 0, 'must', 'seed')"
    ))
    .expect("seed activity");
}

#[test]
fn validate_data_warns_on_activity_status_drift() {
    let _g = VBI_LOCK.lock().unwrap();
    let tag = nanos();
    let plan = format!("zztest-vbi-{tag}");
    let dest = format!("zzvbi{tag}");
    let trip = plan.replace('-', "_");
    let _g = Guard::new({
        let (plan, dest, trip) = (plan.clone(), dest.clone(), trip.clone());
        move || {
            db_exec_teardown_bookings(&trip);
            teardown_plan(&plan, &dest);
        }
    });
    if db_exec("SELECT 1").is_none() {
        eprintln!("skipping (no Turso creds)");
        return;
    }
    seed_plan(&plan, &dest, 0);
    seed_activity(&plan, &dest, &format!("act-{tag}"), "booked");

    // Derive the bookings_current row the regular way, then corrupt its status
    // to reproduce exactly the drift that went undetected for a month.
    let (ok, stdout, stderr) = run_cmd(&["sync-bookings", "--plan-id", &plan]);
    assert!(ok, "sync-bookings should succeed; stdout={stdout} stderr={stderr}");
    db_exec(&format!(
        "UPDATE bookings_current SET status='pending' WHERE trip_id='{trip}'"
    ))
    .expect("corrupt status");

    let (ok, stdout, stderr) = run_cmd(&["validate", "data"]);
    assert!(ok, "drift is a warning, not a failure: stdout tail…{}", stdout.lines().last().unwrap_or(""));
    let _ = stderr;
    let hit = stdout
        .lines()
        .any(|l| l.contains("[bookings]") && l.contains(&plan) && l.contains("status: plan=booked db=pending"));
    assert!(
        hit,
        "validate data must emit a [bookings] warning naming the plan and the status drift; got:\n{}",
        stdout.lines().filter(|l| l.contains("bookings")).collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn validate_data_warns_on_missing_derived_booking_row() {
    let _g = VBI_LOCK.lock().unwrap();
    let tag = nanos();
    let plan = format!("zztest-vbi-{tag}");
    let dest = format!("zzvbi{tag}");
    let trip = plan.replace('-', "_");
    let _g = Guard::new({
        let (plan, dest, trip) = (plan.clone(), dest.clone(), trip.clone());
        move || {
            db_exec_teardown_bookings(&trip);
            teardown_plan(&plan, &dest);
        }
    });
    if db_exec("SELECT 1").is_none() {
        eprintln!("skipping (no Turso creds)");
        return;
    }
    seed_plan(&plan, &dest, 0);
    seed_activity(&plan, &dest, &format!("act-{tag}"), "booked");
    // Deliberately NO sync-bookings run: the derived row is missing.

    let (ok, stdout, _stderr) = run_cmd(&["validate", "data"]);
    assert!(
        ok,
        "missing derived row is a warning, not a failure; stdout:\n{}",
        stdout.lines().filter(|l| l.contains("bookings") || l.contains("Error")).collect::<Vec<_>>().join("\n")
    );
    let hit = stdout
        .lines()
        .any(|l| l.contains("[bookings]") && l.contains(&plan) && l.contains("plan-only"));
    assert!(
        hit,
        "validate data must emit a [bookings] plan-only warning for the unsynced activity; got:\n{}",
        stdout.lines().filter(|l| l.contains("bookings")).collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn validate_data_does_not_flag_accommodation_rows() {
    let _g = VBI_LOCK.lock().unwrap();
    let tag = nanos();
    let plan = format!("zztest-vbi-{tag}");
    let dest = format!("zzvbi{tag}");
    let trip = plan.replace('-', "_");
    let _g = Guard::new({
        let (plan, dest, trip) = (plan.clone(), dest.clone(), trip.clone());
        move || {
            db_exec_teardown_bookings(&trip);
            teardown_plan(&plan, &dest);
        }
    });
    if db_exec("SELECT 1").is_none() {
        eprintln!("skipping (no Turso creds)");
        return;
    }
    seed_plan(&plan, &dest, 0);
    seed_activity(&plan, &dest, &format!("act-{tag}"), "booked");
    let (ok, stdout, stderr) = run_cmd(&["sync-bookings", "--plan-id", &plan]);
    assert!(ok, "sync-bookings should succeed; stdout={stdout} stderr={stderr}");

    // An accommodation row the plan cannot derive (set-accommodation writes it
    // directly, keyed by the plan id): must NOT be reported as db-only drift.
    db_exec(&format!(
        "INSERT INTO bookings_current \
           (booking_key, trip_id, destination, category, title, status, price_currency) \
         VALUES ('{trip}:accommodation:zz', '{trip}', '{dest}', 'accommodation', \
                 'ZZ Stay', 'booked', 'TWD')"
    ))
    .expect("seed accommodation row");

    let (ok, stdout, _stderr) = run_cmd(&["validate", "data"]);
    assert!(ok, "accommodation rows must not fail validation");
    let hit = stdout
        .lines()
        .any(|l| l.contains("[bookings]") && l.contains(&plan) && l.contains("accommodation"));
    assert!(
        !hit,
        "accommodation-only bookings_current rows are not drift; validate data must stay silent about them, got:\n{}",
        stdout.lines().filter(|l| l.contains("bookings")).collect::<Vec<_>>().join("\n")
    );
}

/// bookings_current / bookings_events / payload children are NOT plan-keyed
/// (trip_id-keyed) — teardown_plan does not cover them; delete locally BEFORE
/// teardown_plan per the test rules. Children are matched by EXACT keys from a
/// subquery (a LIKE '{trip}:%' would treat the `_` in trip ids as a wildcard).
fn db_exec_teardown_bookings(trip: &str) {
    let _ = common::db_exec_teardown(&format!(
        "DELETE FROM bookings_current_payload WHERE booking_key IN \
           (SELECT booking_key FROM bookings_current WHERE trip_id = '{trip}'); \
         DELETE FROM bookings_event_data WHERE booking_key IN \
           (SELECT booking_key FROM bookings_current WHERE trip_id = '{trip}'); \
         DELETE FROM bookings_events WHERE booking_key IN \
           (SELECT booking_key FROM bookings_current WHERE trip_id = '{trip}'); \
         DELETE FROM bookings_current WHERE trip_id = '{trip}'"
    ));
}

/// A NON-EMPTY invalid cost_estimate (e.g. TEXT 'abc') must surface as a
/// LOUD finding — never a silent NULL (a silent NULL lets a later
/// sync-bookings overwrite a stored price) and never a libsql panic
/// (codex review 2026-10-29: the empty-string legacy case is handled, other
/// invalid types are an error).
#[test]
fn validate_data_fails_loud_on_invalid_cost_estimate_type() {
    let _g = VBI_LOCK.lock().unwrap();
    let tag = nanos();
    let plan = format!("zztest-vbi-{tag}");
    let dest = format!("zzvbi{tag}");
    let trip = plan.replace('-', "_");
    let _g = Guard::new({
        let (plan, dest, trip) = (plan.clone(), dest.clone(), trip.clone());
        move || {
            db_exec_teardown_bookings(&trip);
            teardown_plan(&plan, &dest);
        }
    });
    if db_exec("SELECT 1").is_none() {
        eprintln!("skipping (no Turso creds)");
        return;
    }
    seed_plan(&plan, &dest, 0);
    db_exec(&format!(
        "INSERT OR REPLACE INTO activities \
           (id, plan_id, destination, day_number, session_type, sort_order, title, \
            booking_required, booking_status, cost_estimate, is_fixed_time, priority, source) \
         VALUES ('act-{tag}', '{plan}', '{dest}', 1, 'morning', 0, 'ZZ Bad Cost', \
                 1, 'booked', 'abc', 0, 'must', 'seed')"
    ))
    .expect("seed bad-cost activity");

    let (ok, stdout, stderr) = run_cmd(&["validate", "data"]);
    // The invalid cell must be REPORTED (error-class, plan-named) — not
    // panic (validate would die mid-report), not silently read as NULL.
    assert!(
        !stdout.is_empty(),
        "validate data must complete its report instead of panicking; stderr={stderr}"
    );
    let hit = stdout.lines().any(|l| {
        l.contains("[bookings]")
            && l.contains(&plan)
            && (l.contains("failed") || l.contains("invalid") || l.contains("cost"))
    });
    assert!(
        hit,
        "a non-empty invalid cost_estimate must surface as a loud [bookings] finding for the plan; got:\n{}",
        stdout.lines().filter(|l| l.contains("bookings")).collect::<Vec<_>>().join("\n")
    );
    let _ = ok;
}
