//! Real-Turso behavior LOCK for `query-recommendations` — read-only listing of
//! `ai_recommended` items on activities, session_meals, and day_route_segments,
//! scoped by optional filters. Asserts filter symmetry with confirm-recommendations.

use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{bin, db_exec, is_credless, nanos, seed_plan, teardown_plan, Guard};

static LOCK: Mutex<()> = Mutex::new(());

fn sql_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn exec_ok(sql: &str) -> common::Rows {
    db_exec(sql).unwrap_or_else(|| panic!("db exec skipped unexpectedly for SQL: {sql}"))
}

/// Run a view command; returns None on a credless mid-test skip.
fn run_cmd(args: &[&str]) -> Option<(String, String)> {
    let out = Command::new(bin())
        .args(args)
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() && is_credless(&stderr) {
        eprintln!("skipping query-recommendations lock mid-test: {}", stderr.trim());
        return None;
    }
    assert!(
        out.status.success(),
        "travel {args:?} should succeed; stdout={stdout} stderr={stderr}"
    );
    Some((stdout, stderr))
}

/// Run a command expected to FAIL (non-zero exit); returns (stdout, stderr).
/// Distinguishes a real refusal from a credless skip.
fn run_cmd_expect_fail(args: &[&str]) -> Option<(String, String)> {
    let out = Command::new(bin())
        .args(args)
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if is_credless(&stderr) {
        eprintln!("skipping query-recommendations lock mid-test: {}", stderr.trim());
        return None;
    }
    assert!(
        !out.status.success(),
        "travel {args:?} should FAIL but succeeded; stdout={stdout} stderr={stderr}"
    );
    Some((stdout, stderr))
}

fn teardown(plan: &str, dest: &str) {
    teardown_plan(plan, dest);
}

#[test]
fn query_recommendations_read_surface_is_locked() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());

    if db_exec("SELECT 1 AS n").is_none() {
        eprintln!("skipping query-recommendations lock (no Turso creds)");
        return;
    }

    let tag = nanos();
    let plan = format!("zztest-query-rec-{tag}");
    let dest = format!("zzqueryrec_{tag}");

    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown(&plan, &dest)
    });
    teardown(&plan, &dest);
    seed_plan(&plan, &dest, 0);

    let p = sql_lit(&plan);
    let d = sql_lit(&dest);
    let act_in = sql_lit(&format!("{tag}-act-in"));
    let act_out = sql_lit(&format!("{tag}-act-out"));
    let act_day2 = sql_lit(&format!("{tag}-act-day2"));
    let act_conf = sql_lit(&format!("{tag}-act-conf"));

    if db_exec(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type, status, updated_at) \
           VALUES ({p}, {d}, 1, '2026-09-01', 'full', 'draft', '2020-01-01 00:00:00'), \
                  ({p}, {d}, 2, '2026-09-02', 'full', 'draft', '2020-01-01 00:00:00'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type, updated_at) \
           VALUES ({p}, {d}, 1, 'morning', '2020-01-01 00:00:00'), \
                  ({p}, {d}, 1, 'noon', '2020-01-01 00:00:00'), \
                  ({p}, {d}, 1, 'evening', '2020-01-01 00:00:00'), \
                  ({p}, {d}, 2, 'morning', '2020-01-01 00:00:00');"
    ))
    .is_none()
    {
        return;
    }

    if db_exec(&format!(
        "INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title, source, updated_at) \
           VALUES ({act_in}, {p}, {d}, 1, 'morning', 0, 'AI act in', 'ai_recommended', '2020-01-01 00:00:00'), \
                  ({act_out}, {p}, {d}, 1, 'evening', 0, 'AI act out sess', 'ai_recommended', '2020-01-01 00:00:00'), \
                  ({act_day2}, {p}, {d}, 2, 'morning', 0, 'AI act out day', 'ai_recommended', '2020-01-01 00:00:00'), \
                  ({act_conf}, {p}, {d}, 1, 'morning', 1, 'Confirmed act', 'confirmed', '2020-01-01 00:00:00');"
    ))
    .is_none()
    {
        return;
    }

    if db_exec(&format!(
        "INSERT INTO session_meals (plan_id, destination, day_number, session_type, sort_order, meal, source, updated_at) \
           VALUES ({p}, {d}, 1, 'morning', 0, 'AI breakfast in', 'ai_recommended', '2020-01-01 00:00:00'), \
                  ({p}, {d}, 1, 'evening', 0, 'AI dinner out', 'ai_recommended', '2020-01-01 00:00:00'), \
                  ({p}, {d}, 2, 'morning', 0, 'AI breakfast d2', 'ai_recommended', '2020-01-01 00:00:00'), \
                  ({p}, {d}, 1, 'morning', 1, 'Confirmed bfast', 'confirmed', '2020-01-01 00:00:00');"
    ))
    .is_none()
    {
        return;
    }

    if db_exec(&format!(
        "INSERT INTO day_route_segments (plan_id, destination, day_number, sort_order, from_place, to_place, mode, source) \
           VALUES ({p}, {d}, 1, 0, 'A', 'B', 'walking', 'ai_recommended'), \
                  ({p}, {d}, 2, 0, 'C', 'D', 'walking', 'ai_recommended'), \
                  ({p}, {d}, 1, 1, 'E', 'F', 'walking', 'confirmed');"
    ))
    .is_none()
    {
        return;
    }

    // ── Scenario 1: BASE — all ai_recommended items ──
    let out = run_cmd(&[
        "query-recommendations",
        "--plan-id",
        &plan,
        "--dest",
        &dest,
    ]);
    if out.is_none() {
        return;
    }
    let (stdout, _) = out.unwrap();
    assert!(
        stdout.contains("8 AI-recommended item(s) awaiting confirmation (3 activities, 3 meals, 2 routes)"),
        "base query must show full count; got {stdout:?}"
    );
    assert!(stdout.contains("AI act in"), "base must include AI act in");
    assert!(stdout.contains("AI breakfast in"), "base must include AI breakfast in");
    assert!(stdout.contains("A -> B"), "base must include A -> B route");
    assert!(!stdout.contains("Confirmed act"), "base must exclude confirmed act");
    assert!(!stdout.contains("Confirmed bfast"), "base must exclude confirmed bfast");
    assert!(!stdout.contains("E -> F"), "base must exclude confirmed route");

    // ── Scenario 2: DAY+SESSION --day 1 --session morning ──
    let out2 = run_cmd(&[
        "query-recommendations",
        "--day",
        "1",
        "--session",
        "morning",
        "--plan-id",
        &plan,
        "--dest",
        &dest,
    ]);
    if out2.is_none() {
        return;
    }
    let (stdout2, _) = out2.unwrap();
    assert!(
        stdout2.contains("3 AI-recommended item(s) awaiting confirmation (1 activities, 1 meals, 1 routes)"),
        "day+session query must show scoped count; got {stdout2:?}"
    );
    assert!(stdout2.contains("AI act in"));
    assert!(stdout2.contains("AI breakfast in"));
    assert!(stdout2.contains("A -> B"));
    assert!(!stdout2.contains("AI act out sess"));
    assert!(!stdout2.contains("AI dinner out"));
    assert!(!stdout2.contains("AI act out day"));
    assert!(!stdout2.contains("AI breakfast d2"));
    assert!(!stdout2.contains("C -> D"));

    // ── Scenario 3: KIND --kind meal ──
    let out3 = run_cmd(&[
        "query-recommendations",
        "--kind",
        "meal",
        "--plan-id",
        &plan,
        "--dest",
        &dest,
    ]);
    if out3.is_none() {
        return;
    }
    let (stdout3, _) = out3.unwrap();
    assert!(
        stdout3.contains("3 AI-recommended item(s) awaiting confirmation (0 activities, 3 meals, 0 routes)"),
        "meal-only query must show meal count; got {stdout3:?}"
    );
    assert!(stdout3.contains("AI breakfast in"));
    assert!(stdout3.contains("AI dinner out"));
    assert!(stdout3.contains("AI breakfast d2"));
    assert!(!stdout3.contains("AI act in"));
    assert!(!stdout3.contains("A -> B"));

    // ── Scenario 4: GUARD --kind route --session morning ──
    let fail_out = run_cmd_expect_fail(&[
        "query-recommendations",
        "--kind",
        "route",
        "--session",
        "morning",
        "--plan-id",
        &plan,
        "--dest",
        &dest,
    ]);
    if fail_out.is_none() {
        return;
    }
    let (_, stderr4) = fail_out.unwrap();
    assert!(
        stderr4.contains("--session cannot be used with --kind route"),
        "stderr must contain route/session guard message; got {stderr4:?}"
    );

    // ── Scenario 5: EMPTY --day 99 ──
    let out5 = run_cmd(&[
        "query-recommendations",
        "--day",
        "99",
        "--plan-id",
        &plan,
        "--dest",
        &dest,
    ]);
    if out5.is_none() {
        return;
    }
    let (stdout5, _) = out5.unwrap();
    assert!(
        stdout5.contains("No AI-recommended items awaiting confirmation."),
        "empty scope must print no-items message; got {stdout5:?}"
    );

    // read-only: no operation_runs rows written
    let audit_count = exec_ok(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id = {p}"
    ));
    assert_eq!(
        audit_count.scalar().as_deref(),
        Some("0"),
        "read-only query must not write operation_runs"
    );
}