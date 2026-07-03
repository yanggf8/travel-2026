//! Integration port of the `StateManager.setActivityTime` block from
//! tests/integration/state-manager.regression.test.ts.
//!
//! The TS test drives `StateManager.setActivityTime()` against an in-memory
//! plan. The Rust equivalent is `travel set-activity-time <day> <session>
//! <activity> [--start] [--end] [--fixed]`, which UPDATEs the `activities` row
//! (by exact id OR case-insensitive title substring) and emits an
//! `activity_time_updated` plan_event with from/to diff.
//!
//! Ported setActivityTime cases:
//!   1. finds activity by exact ID  → start_time written
//!   2. finds activity by title substring (case-insensitive, lower)  → written
//!   3. finds activity by title substring (UPPER case)  → written
//!   4. only updates provided fields, preserves others (set start, then end)
//!   5. allows setting is_fixed_time to false explicitly (true → false)
//!   6. emits activity_time_updated event with from/to diff
//!
//! (The TS legacy-string-upgrade and derivePlanId cases are TS-only concepts:
//!  the Rust path always operates on a real normalized `activities` row, and
//!  derivePlanId is exercised by plan_resolver.rs unit tests.)
//!
//! Each test seeds a throwaway plan/day/session/activity, runs the command,
//! asserts on the `activities` readback AND the `plan_events` / `plan_event_data`
//! rows, then tears everything down. Unique namespaced ids avoid collisions and
//! never touch real plans.

use std::process::Command;

mod common;
use common::{bin, db_exec, db_exec_teardown, nanos, seed_plan, teardown_plan, Guard};

/// Seed plan + plan_metadata + one day with a single activity "Hotel checkout"
/// (id `act-checkout-<plan_id>`, suffixed because `activities.id` is the global
/// PRIMARY KEY) in the morning session. Mirrors the day-5 fixture row.
/// Returns false on credless skip.
fn seed(plan_id: &str, dest: &str) -> bool {
    if db_exec("SELECT 1").is_none() {
        return false;
    }
    seed_plan(plan_id, dest, 0);
    let act_id = format!("act-checkout-{plan_id}");
    let sql = format!(
        "INSERT INTO days (plan_id, destination, day_number, date, theme, day_type, status) \
           VALUES ('{plan_id}', '{dest}', 5, '2026-02-17', 'Checkout', 'departure', 'draft'); \
         INSERT INTO activities \
           (id, plan_id, destination, day_number, session_type, sort_order, title, \
            duration_min, booking_required, is_fixed_time, priority) \
         VALUES ('{act_id}', '{plan_id}', '{dest}', 5, 'morning', 0, 'Hotel checkout', \
            30, 0, 0, 'want');"
    );
    db_exec(&sql).is_some()
}

fn teardown(plan_id: &str, dest: &str) {
    let _ = db_exec_teardown(&format!(
        "DELETE FROM activity_tags WHERE activity_id LIKE 'act-checkout-{plan_id}';"
    ));
    teardown_plan(plan_id, dest);
}

/// Run set-activity-time. Returns (success, stdout, stderr).
fn set_time(plan_id: &str, dest: &str, day: &str, session: &str, activity: &str, extra: &[&str]) -> (bool, String, String) {
    let mut args: Vec<String> = vec![
        "set-activity-time".to_string(),
        day.to_string(),
        session.to_string(),
        activity.to_string(),
        "--dest".to_string(),
        dest.to_string(),
    ];
    for e in extra {
        args.push((*e).to_string());
    }
    let out = Command::new(bin())
        .args(&args)
        .env("TRAVEL_PLAN_ID", plan_id)
        .output()
        .expect("run travel set-activity-time");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Read one activity scalar column for the seeded act-checkout row.
fn read_activity_col(plan_id: &str, dest: &str, col: &str) -> String {
    let act_id = format!("act-checkout-{plan_id}");
    // No column alias: db exec renders rows as `<col>: <value>`, so the bare
    // column name shows up verbatim in the assertion text below.
    let sql = format!(
        "SELECT {col} FROM activities WHERE plan_id = '{plan_id}' AND destination = '{dest}' AND id = '{act_id}'"
    );
    db_exec(&sql)
        .map(|rows| rows.raw().to_string())
        .unwrap_or_default()
}

// ── 1. exact ID match ───────────────────────────────────────────────────────
#[tokio::test]
async fn finds_activity_by_exact_id() {
    let tag = nanos();
    let plan_id = format!("test-plan-statemgr-id-{tag}");
    let dest = format!("statemgrid_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown(&plan_id, &dest)
    });
    if !seed(&plan_id, &dest) {
        return;
    }

    let (ok, stdout, stderr) = set_time(&plan_id, &dest, "5", "morning", &format!("act-checkout-{plan_id}"), &["--start", "11:00"]);
    let start = read_activity_col(&plan_id, &dest, "start_time");

    assert!(ok, "set-activity-time by id should succeed; stdout={stdout} stderr={stderr}");
    assert!(start.contains("start_time: 11:00"), "start_time should be 11:00; got {start}");
}

// ── 2. title substring, lowercase ───────────────────────────────────────────
#[tokio::test]
async fn finds_activity_by_title_substring_lowercase() {
    let tag = nanos();
    let plan_id = format!("test-plan-statemgr-lc-{tag}");
    let dest = format!("statemgrlc_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown(&plan_id, &dest)
    });
    if !seed(&plan_id, &dest) {
        return;
    }

    let (ok, stdout, stderr) = set_time(&plan_id, &dest, "5", "morning", "checkout", &["--start", "11:00"]);
    let start = read_activity_col(&plan_id, &dest, "start_time");

    assert!(ok, "set-activity-time by lowercase substring should succeed; stdout={stdout} stderr={stderr}");
    assert!(start.contains("start_time: 11:00"), "start_time should be 11:00; got {start}");
}

// ── 3. title substring, uppercase (case-insensitive) ────────────────────────
#[tokio::test]
async fn finds_activity_by_title_substring_uppercase() {
    let tag = nanos();
    let plan_id = format!("test-plan-statemgr-uc-{tag}");
    let dest = format!("statemgruc_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown(&plan_id, &dest)
    });
    if !seed(&plan_id, &dest) {
        return;
    }

    let (ok, stdout, stderr) = set_time(&plan_id, &dest, "5", "morning", "CHECKOUT", &["--start", "11:00"]);
    let start = read_activity_col(&plan_id, &dest, "start_time");

    assert!(ok, "set-activity-time by uppercase substring should succeed; stdout={stdout} stderr={stderr}");
    assert!(start.contains("start_time: 11:00"), "start_time should be 11:00; got {start}");
}

// ── 4. only updates provided fields, preserves others ───────────────────────
#[tokio::test]
async fn only_updates_provided_fields() {
    let tag = nanos();
    let plan_id = format!("test-plan-statemgr-partial-{tag}");
    let dest = format!("statemgrpartial_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown(&plan_id, &dest)
    });
    if !seed(&plan_id, &dest) {
        return;
    }

    // First set start_time.
    let (ok1, _o1, e1) = set_time(&plan_id, &dest, "5", "morning", "checkout", &["--start", "11:00"]);
    assert!(ok1, "first set-activity-time should succeed; stderr={e1}");
    // Then set end_time only — start_time must be preserved.
    let (ok2, _o2, e2) = set_time(&plan_id, &dest, "5", "morning", "checkout", &["--end", "11:30"]);

    let start = read_activity_col(&plan_id, &dest, "start_time");
    let end = read_activity_col(&plan_id, &dest, "end_time");

    assert!(ok2, "second set-activity-time should succeed; stderr={e2}");
    assert!(start.contains("start_time: 11:00"), "start_time should be preserved at 11:00; got {start}");
    assert!(end.contains("end_time: 11:30"), "end_time should be 11:30; got {end}");
}

// ── 5. set is_fixed_time to false explicitly ────────────────────────────────
#[tokio::test]
async fn sets_is_fixed_time_false_explicitly() {
    let tag = nanos();
    let plan_id = format!("test-plan-statemgr-fixed-{tag}");
    let dest = format!("statemgrfixed_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown(&plan_id, &dest)
    });
    if !seed(&plan_id, &dest) {
        return;
    }

    // true first.
    let (ok1, _o1, e1) = set_time(&plan_id, &dest, "5", "morning", "checkout", &["--fixed", "true"]);
    assert!(ok1, "set fixed=true should succeed; stderr={e1}");
    let fixed_true = read_activity_col(&plan_id, &dest, "is_fixed_time");
    assert!(fixed_true.contains("is_fixed_time: 1"), "is_fixed_time should be 1 (true); got {fixed_true}");

    // false explicitly — must not be ignored.
    let (ok2, _o2, e2) = set_time(&plan_id, &dest, "5", "morning", "checkout", &["--fixed", "false"]);
    let fixed_false = read_activity_col(&plan_id, &dest, "is_fixed_time");

    assert!(ok2, "set fixed=false should succeed; stderr={e2}");
    assert!(fixed_false.contains("is_fixed_time: 0"), "is_fixed_time should be 0 (false); got {fixed_false}");
}

// ── 6. emits activity_time_updated event with from/to diff ──────────────────
#[tokio::test]
async fn emits_activity_time_updated_event() {
    let tag = nanos();
    let plan_id = format!("test-plan-statemgr-event-{tag}");
    let dest = format!("statemgrevent_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown(&plan_id, &dest)
    });
    if !seed(&plan_id, &dest) {
        return;
    }

    let (ok, stdout, stderr) = set_time(&plan_id, &dest, "5", "morning", "checkout", &["--start", "11:00", "--fixed", "true"]);
    assert!(ok, "set-activity-time should succeed; stdout={stdout} stderr={stderr}");

    // Assert the timeline-scope plan_events row exists for this event.
    let events = db_exec(&format!(
        "SELECT event FROM plan_events WHERE plan_id = '{plan_id}' AND scope = 'timeline' AND event = 'activity_time_updated'"
    ))
    .map(|rows| rows.raw().to_string())
    .unwrap_or_default();
    assert!(
        events.contains("event: activity_time_updated"),
        "expected an activity_time_updated event; got {events}"
    );

    // Assert the event payload (plan_event_data) carries the diff fields.
    let kv = db_exec(&format!(
        "SELECT key, value FROM plan_event_data WHERE plan_id = '{plan_id}' AND scope = 'timeline' ORDER BY key"
    ))
    .map(|rows| rows.raw().to_string())
    .unwrap_or_default();

    // day_number = 5, session = morning, activity_id = act-checkout-<plan>.
    assert!(kv.contains("key: day_number, value: 5"), "event should carry day_number=5; got {kv}");
    assert!(kv.contains("key: session, value: morning"), "event should carry session=morning; got {kv}");
    assert!(
        kv.contains(&format!("key: activity_id, value: act-checkout-{plan_id}")),
        "event should carry the activity_id; got {kv}"
    );
    // from: start_time undefined → to: start_time=11:00 + is_fixed_time=true.
    assert!(
        kv.contains("key: from, value: start_time=undefined"),
        "from-diff should show start_time=undefined; got {kv}"
    );
    assert!(
        kv.contains("start_time=11:00") && kv.contains("is_fixed_time=true"),
        "to-diff should show start_time=11:00 and is_fixed_time=true; got {kv}"
    );
}