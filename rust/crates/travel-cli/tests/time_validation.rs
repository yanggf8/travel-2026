//! Real-Turso integration tests for fail-loud HH:MM time validation in the
//! time-writing commands: `set-activity-time`, `add-activity`,
//! `set-tod-time-range`, and `set-flight --dep/--arr`.
//!
//! Each proves the WRITE-TIME guard: a bad time is REJECTED (non-zero exit, no
//! domain row written), a good time PASSES (row persisted), and an inverted
//! start>end range is REJECTED. Pattern mirrors set_mutation_bugs.rs: seed a
//! throwaway plan, run the binary, SELECT to assert, tear down. Skips cleanly
//! when Turso creds are absent.
//!
//! NEVER touches okinawa-2026 / tokyo-2026 / kyoto-2026 — every plan id here is
//! `test-timeval-*` + a nanosecond tag, removed in teardown.

use std::process::Command;

mod common;
use common::{bin, db_exec, db_exec_teardown, nanos, seed_plan, teardown_plan, Guard};

/// Seed a plan with a day 1, a `morning` timesofday session, and one activity
/// titled "checkout" so set-activity-time can resolve it. Returns false on a
/// credless skip.
fn seed_full(plan_id: &str, dest: &str) -> bool {
    if db_exec("SELECT 1").is_none() {
        return false;
    }
    seed_plan(plan_id, dest, 0);
    let sql = format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type, updated_at) \
           VALUES ('{plan_id}', '{dest}', 1, '2026-06-12', 'full', datetime('now')); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type, updated_at) \
           VALUES ('{plan_id}', '{dest}', 1, 'morning', datetime('now')); \
         INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title, updated_at) \
           VALUES ('{plan_id}-act1', '{plan_id}', '{dest}', 1, 'morning', 0, 'checkout', datetime('now'));"
    );
    db_exec(&sql).is_some()
}

fn teardown(plan_id: &str, dest: &str) {
    let _ = db_exec_teardown(&format!(
        "DELETE FROM activity_tags WHERE activity_id LIKE '{plan_id}%';"
    ));
    teardown_plan(plan_id, dest);
}

/// Run a `travel` subcommand with TRAVEL_PLAN_ID set. Returns (ok, stdout, stderr).
fn run_cmd(plan_id: &str, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(args)
        .env("TRAVEL_PLAN_ID", plan_id)
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// COUNT(*) helper. Returns the integer count, or None on a credless skip.
fn count(sql: &str) -> Option<i64> {
    let n = db_exec(sql)?
        .scalar()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    Some(n)
}

// ── set-activity-time: bad start rejected (no audit), good start persisted ──
#[test]
fn set_activity_time_rejects_bad_time_writes_nothing() {
    let tag = nanos();
    let plan_id = format!("test-timeval-act-{tag}");
    let dest = format!("timevalact_{tag}");
    if !seed_full(&plan_id, &dest) {
        return;
    }
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown(&plan_id, &dest)
    });

    // 1. Bad time → non-zero, actionable error, NOTHING written.
    let (ok, _stdout, stderr) = run_cmd(
        &plan_id,
        &["set-activity-time", "1", "morning", "checkout", "--start", "9am", "--dest", &dest],
    );
    let audit_bad = count(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs \
         WHERE plan_id = '{plan_id}' AND command_type = 'set-activity-time' AND status = 'completed'"
    ));
    let start_after_bad = db_exec(&format!(
        "SELECT start_time FROM activities WHERE plan_id = '{plan_id}' AND id = '{plan_id}-act1'"
    ))
    .map(|rows| rows.raw().to_string());

    // 2. Good time → succeeds and persists.
    let (ok_good, _o2, e2) = run_cmd(
        &plan_id,
        &["set-activity-time", "1", "morning", "checkout", "--start", "09:00", "--dest", &dest],
    );
    let start_after_good = db_exec(&format!(
        "SELECT start_time FROM activities WHERE plan_id = '{plan_id}' AND id = '{plan_id}-act1'"
    ))
    .map(|rows| rows.raw().to_string());

    assert!(
        !ok,
        "set-activity-time must exit non-zero on a bad time; stderr={stderr}"
    );
    assert!(
        stderr.contains("--start") && stderr.contains("9am"),
        "error must name the bad flag and value; got: {stderr}"
    );
    assert_eq!(
        audit_bad,
        Some(0),
        "no completed audit row may be written when the time was rejected"
    );
    // start_time must remain unset (NULL renders as no/empty value) after the
    // rejected write.
    assert!(
        !start_after_bad.clone().unwrap_or_default().contains("start_time: 09:00"),
        "the rejected write must not have set start_time; got: {start_after_bad:?}"
    );

    assert!(ok_good, "a valid --start must succeed; stderr={e2}");
    assert!(
        start_after_good.unwrap_or_default().contains("start_time: 09:00"),
        "the valid --start must persist"
    );
}

// ── set-activity-time: start>end rejected, nothing written ──────────────────
#[test]
fn set_activity_time_rejects_start_after_end() {
    let tag = nanos();
    let plan_id = format!("test-timeval-actrange-{tag}");
    let dest = format!("timevalactrange_{tag}");
    if !seed_full(&plan_id, &dest) {
        return;
    }
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown(&plan_id, &dest)
    });

    let (ok, _stdout, stderr) = run_cmd(
        &plan_id,
        &[
            "set-activity-time", "1", "morning", "checkout",
            "--start", "14:00", "--end", "09:00", "--dest", &dest,
        ],
    );
    let audit = count(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs \
         WHERE plan_id = '{plan_id}' AND command_type = 'set-activity-time' AND status = 'completed'"
    ));

    assert!(!ok, "start>end must exit non-zero; stderr={stderr}");
    assert!(
        stderr.contains("start must be"),
        "error must explain the start≤end rule; got: {stderr}"
    );
    assert_eq!(audit, Some(0), "no audit row when the range was rejected");
}

// ── add-activity: bad end rejected (no row), good times persist ─────────────
#[test]
fn add_activity_rejects_bad_time_writes_nothing() {
    let tag = nanos();
    let plan_id = format!("test-timeval-add-{tag}");
    let dest = format!("timevaladd_{tag}");
    if !seed_full(&plan_id, &dest) {
        return;
    }
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown(&plan_id, &dest)
    });

    // Bad time → non-zero, no new activity row.
    let (ok, _o, stderr) = run_cmd(
        &plan_id,
        &["add-activity", "1", "morning", "Coffee run", "--end", "noon", "--dest", &dest],
    );
    let count_after_bad = count(&format!(
        "SELECT COUNT(*) AS n FROM activities \
         WHERE plan_id = '{plan_id}' AND title = 'Coffee run'"
    ));

    // Good times → row inserted.
    let (ok_good, _o2, e2) = run_cmd(
        &plan_id,
        &[
            "add-activity", "1", "morning", "Coffee run",
            "--start", "08:00", "--end", "08:30", "--dest", &dest,
        ],
    );
    let count_after_good = count(&format!(
        "SELECT COUNT(*) AS n FROM activities \
         WHERE plan_id = '{plan_id}' AND title = 'Coffee run'"
    ));

    assert!(!ok, "add-activity must exit non-zero on a bad time; stderr={stderr}");
    assert!(
        stderr.contains("--end") && stderr.contains("noon"),
        "error must name the bad flag and value; got: {stderr}"
    );
    assert_eq!(
        count_after_bad,
        Some(0),
        "no activity row may be inserted when the time was rejected"
    );

    assert!(ok_good, "valid times must succeed; stderr={e2}");
    assert_eq!(
        count_after_good,
        Some(1),
        "the activity must be inserted once times are valid"
    );
}

// ── set-tod-time-range: bad/inverted rejected, valid persisted ──────────────
#[test]
fn set_tod_time_range_validates_times() {
    let tag = nanos();
    let plan_id = format!("test-timeval-tod-{tag}");
    let dest = format!("timevaltod_{tag}");
    if !seed_full(&plan_id, &dest) {
        return;
    }
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown(&plan_id, &dest)
    });

    // Bad start.
    let (ok_bad, _o, e_bad) = run_cmd(
        &plan_id,
        &["set-tod-time-range", "1", "morning", "--start", "8am", "--end", "12:00", "--dest", &dest],
    );
    // Inverted range.
    let (ok_inv, _o2, e_inv) = run_cmd(
        &plan_id,
        &["set-tod-time-range", "1", "morning", "--start", "13:00", "--end", "09:00", "--dest", &dest],
    );
    let audit_after_bad = count(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs \
         WHERE plan_id = '{plan_id}' AND command_type = 'set-session-time-range' AND status = 'completed'"
    ));

    // Valid range → persisted.
    let (ok_good, _o3, e_good) = run_cmd(
        &plan_id,
        &["set-tod-time-range", "1", "morning", "--start", "08:00", "--end", "12:00", "--dest", &dest],
    );
    let stored = db_exec(&format!(
        "SELECT time_range_start, time_range_end FROM timesofday \
         WHERE plan_id = '{plan_id}' AND day_number = 1 AND session_type = 'morning'"
    ))
    .map(|rows| rows.raw().to_string());

    assert!(!ok_bad, "bad --start must exit non-zero; stderr={e_bad}");
    assert!(e_bad.contains("--start") && e_bad.contains("8am"), "got: {e_bad}");
    assert!(!ok_inv, "inverted range must exit non-zero; stderr={e_inv}");
    assert!(e_inv.contains("start must be"), "got: {e_inv}");
    assert_eq!(
        audit_after_bad,
        Some(0),
        "no audit row may be written for the rejected time ranges"
    );

    assert!(ok_good, "valid range must succeed; stderr={e_good}");
    let stored = stored.unwrap_or_default();
    assert!(
        stored.contains("time_range_start: 08:00") && stored.contains("time_range_end: 12:00"),
        "the valid range must persist; got: {stored}"
    );
}

// ── set-flight: bad --dep rejected (no leg), good --dep/--arr persisted ─────
#[test]
fn set_flight_validates_dep_arr_times() {
    let tag = nanos();
    let plan_id = format!("test-timeval-flight-{tag}");
    let dest = format!("timevalflight_{tag}");
    if db_exec("SELECT 1").is_none() {
        return;
    }
    seed_plan(&plan_id, &dest, 0);
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown(&plan_id, &dest)
    });

    // Bad --dep → non-zero, no leg row.
    let (ok_bad, _o, e_bad) = run_cmd(
        &plan_id,
        &[
            "set-flight", "outbound", "--dest", &dest,
            "--flight", "CI120", "--from", "TPE", "--dep", "9am", "--to", "OKA", "--arr", "10:45",
        ],
    );
    let leg_after_bad = count(&format!(
        "SELECT COUNT(*) AS n FROM flight_legs WHERE plan_id = '{plan_id}' AND direction = 'outbound'"
    ));

    // Good times → leg persisted.
    let (ok_good, _o2, e_good) = run_cmd(
        &plan_id,
        &[
            "set-flight", "outbound", "--dest", &dest,
            "--flight", "CI120", "--from", "TPE", "--dep", "08:00", "--to", "OKA", "--arr", "10:45",
            "--date", "2026-06-12",
        ],
    );
    let dep_after_good = db_exec(&format!(
        "SELECT departure_time FROM flight_legs WHERE plan_id = '{plan_id}' AND direction = 'outbound'"
    ))
    .map(|rows| rows.raw().to_string());

    assert!(!ok_bad, "bad --dep must exit non-zero; stderr={e_bad}");
    assert!(e_bad.contains("--dep") && e_bad.contains("9am"), "got: {e_bad}");
    assert_eq!(
        leg_after_bad,
        Some(0),
        "no flight_legs row may be written when --dep was rejected"
    );

    assert!(ok_good, "valid --dep/--arr must succeed; stderr={e_good}");
    assert!(
        dep_after_good.unwrap_or_default().contains("departure_time: 08:00"),
        "the valid departure_time must persist"
    );
}