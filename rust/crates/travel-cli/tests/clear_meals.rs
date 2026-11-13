//! Real-Turso test for `clear-meals`: deletes every meal of one session, writes the
//! audit triad (plan_events, +1 plans.version, operation_runs); an empty session is a
//! note-only no-op (no version bump, no operation_runs row).

use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{bin, db_exec, is_credless, nanos, seed_plan, teardown_plan, Guard};

static LOCK: Mutex<()> = Mutex::new(());

fn exec_ok(sql: &str) -> common::Rows {
    db_exec(sql).unwrap_or_else(|| panic!("db exec skipped unexpectedly for SQL: {sql}"))
}

/// Returns None on a credless mid-test skip.
fn run_cmd(args: &[&str]) -> Option<String> {
    let out = Command::new(bin())
        .args(args)
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() && is_credless(&stderr) {
        eprintln!("skipping clear-meals test mid-test: {}", stderr.trim());
        return None;
    }
    assert!(out.status.success(), "travel {args:?} failed; stdout={stdout} stderr={stderr}");
    Some(stdout)
}

fn n(sql: &str) -> String {
    exec_ok(sql).scalar().unwrap_or_default()
}

#[test]
fn clear_meals_deletes_session_meals_and_audits() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if db_exec("SELECT 1 AS n").is_none() {
        eprintln!("skipping clear-meals test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_plan(&plan, &dest, 0);
    let (p, d) = (format!("'{plan}'"), format!("'{dest}'"));
    exec_ok(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type, status, updated_at) \
           VALUES ({p}, {d}, 1, '2026-09-01', 'full', 'draft', '2020-01-01 00:00:00'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type, updated_at) \
           VALUES ({p}, {d}, 1, 'noon', '2020-01-01 00:00:00'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type, updated_at) \
           VALUES ({p}, {d}, 1, 'evening', '2020-01-01 00:00:00'); \
         INSERT INTO days (plan_id, destination, day_number, date, day_type, status, updated_at) \
           VALUES ({p}, {d}, 2, '2026-09-02', 'full', 'draft', '2020-01-01 00:00:00'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type, updated_at) \
           VALUES ({p}, {d}, 2, 'noon', '2020-01-01 00:00:00');"
    ));

    let meals_on = |day: i64, session: &str| {
        n(&format!(
            "SELECT COUNT(*) AS n FROM session_meals WHERE plan_id = {p} AND destination = {d} \
             AND day_number = {day} AND session_type = '{session}'"
        ))
    };
    let meals = |session: &str| meals_on(1, session);
    let version = || n(&format!("SELECT version AS v FROM plans WHERE plan_id = {p}"));
    let ops = |cmd: &str| {
        n(&format!(
            "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id = {p} AND command_type = '{cmd}'"
        ))
    };

    for (session, a, b) in [("noon", "Lunch: A", "Lunch: B"), ("evening", "Dinner: C", "Dinner: D")] {
        if run_cmd(&["set-meals", "1", session, "--meal", a, "--meal", b, "--plan-id", &plan, "--dest", &dest]).is_none() {
            return;
        }
    }
    if run_cmd(&["set-meals", "2", "noon", "--meal", "Lunch: E", "--plan-id", &plan, "--dest", &dest]).is_none() {
        return;
    }
    assert_eq!(meals("noon"), "2");
    assert_eq!(meals_on(2, "noon"), "1");
    assert_eq!(version(), "3");

    let Some(out) = run_cmd(&["clear-meals", "1", "noon", "--plan-id", &plan, "--dest", &dest]) else {
        return;
    };
    assert!(out.contains("✅ Cleared 2 meal(s) for day 1 session noon"), "stdout={out}");
    assert_eq!(meals("noon"), "0", "noon meals must be gone");
    assert_eq!(meals("evening"), "2", "other session untouched");
    assert_eq!(meals_on(2, "noon"), "1", "same session on another day untouched");
    assert_eq!(version(), "4", "plans.version +1");
    assert_eq!(ops("clear-meals"), "1", "one operation_runs row");
    assert_eq!(
        n(&format!(
            "SELECT version_before || '>' || version_after || '|' || status AS v FROM operation_runs \
             WHERE plan_id = {p} AND command_type = 'clear-meals'"
        )),
        "3>4|completed"
    );
    assert_eq!(
        n(&format!(
            "SELECT COUNT(*) AS n FROM plan_events WHERE plan_id = {p} AND event = 'itinerary_session_meals_cleared'"
        )),
        "2",
        "dest_process + timeline events"
    );

    // plan_event_data for the clear event: 3 keys x (dest_process + timeline) scopes.
    let kv = |scope: &str| {
        n(&format!(
            "SELECT group_concat(d.key || '=' || d.value, ',') AS v FROM \
               (SELECT key, value FROM plan_event_data x WHERE x.plan_id = {p} AND x.scope = '{scope}' \
                  AND EXISTS (SELECT 1 FROM plan_events e WHERE e.plan_id = x.plan_id AND e.scope = x.scope \
                    AND e.destination = x.destination AND e.process_id = x.process_id \
                    AND e.sort_order = x.sort_order AND e.event = 'itinerary_session_meals_cleared') \
                ORDER BY key) d"
        ))
    };
    for scope in ["dest_process", "timeline"] {
        assert_eq!(kv(scope), "count=2,day_number=1,session=noon", "plan_event_data ({scope})");
    }

    // Empty session: note only, no bump, no new operation row.
    let Some(out) = run_cmd(&["clear-meals", "1", "noon", "--plan-id", &plan, "--dest", &dest]) else {
        return;
    };
    assert!(out.contains("No meals to clear"), "stdout={out}");
    assert_eq!(version(), "4", "no bump when nothing to clear");
    assert_eq!(ops("clear-meals"), "1", "no extra operation_runs row");
}
