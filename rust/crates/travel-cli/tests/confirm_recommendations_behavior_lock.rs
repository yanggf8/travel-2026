//! Real-Turso behavior LOCK for `confirm-recommendations` — flips
//! `ai_recommended` → `confirmed` on activities, session_meals, and
//! day_route_segments, scoped by optional filters. Asserts write surface,
//! zero-scope no-op (no audit), and route/session contradiction guard.

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

/// Run a mutation command; returns None on a credless mid-test skip.
fn run_cmd(args: &[&str]) -> Option<(String, String)> {
    let out = Command::new(bin())
        .args(args)
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() && is_credless(&stderr) {
        eprintln!("skipping confirm-recommendations lock mid-test: {}", stderr.trim());
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
        eprintln!("skipping confirm-recommendations lock mid-test: {}", stderr.trim());
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

fn assert_audit(
    plan: &str,
    expected_count: i64,
    expected_version: i64,
    command_type: &str,
    cmd_type_rows: i64,
) {
    let p = sql_lit(plan);
    let count = exec_ok(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id = {p}"
    ));
    assert_eq!(
        count.scalar().as_deref(),
        Some(expected_count.to_string().as_str()),
        "operation_runs total count after {command_type}"
    );

    let version = exec_ok(&format!("SELECT version AS v FROM plans WHERE plan_id = {p}"));
    assert_eq!(
        version.scalar().as_deref(),
        Some(expected_version.to_string().as_str()),
        "plans.version after {command_type}"
    );

    let cmd = sql_lit(command_type);
    let cmd_count = exec_ok(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id = {p} AND command_type = {cmd}"
    ));
    assert_eq!(
        cmd_count.scalar().as_deref(),
        Some(cmd_type_rows.to_string().as_str()),
        "operation_runs row count for {command_type}"
    );
    let row = exec_ok(&format!(
        "SELECT version_before || '>' || COALESCE(version_after, -1) || '|' || status AS v \
         FROM operation_runs WHERE plan_id = {p} AND command_type = {cmd} \
         ORDER BY version_after DESC LIMIT 1"
    ));
    assert_eq!(
        row.scalar().as_deref(),
        Some(format!("{}>{expected_version}|completed", expected_version - 1).as_str()),
        "latest completed operation row for {command_type} records the +1 bump; out={row}"
    );
}

#[test]
fn confirm_recommendations_write_surface_is_locked() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());

    if db_exec("SELECT 1 AS n").is_none() {
        eprintln!("skipping confirm-recommendations lock (no Turso creds)");
        return;
    }

    let tag = nanos();
    let plan = format!("zztest-confirm-{tag}");
    let dest = format!("zzconfirm_{tag}");

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

    // ── Scenario 1: scoped all-kinds --day 1 --session morning ──
    let out = run_cmd(&[
        "confirm-recommendations",
        "--day",
        "1",
        "--session",
        "morning",
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
        stdout.contains("✅ 3 confirmed (1 activities, 1 meals, 1 routes)"),
        "stdout must show the house-style confirm summary; got {stdout:?}"
    );

    let acts = exec_ok(&format!(
        "SELECT id || '|' || source AS v FROM activities WHERE plan_id = {p} AND destination = {d} ORDER BY id"
    ));
    assert_eq!(
        acts.column(),
        vec![
            format!("{tag}-act-conf|confirmed"),
            format!("{tag}-act-day2|ai_recommended"),
            format!("{tag}-act-in|confirmed"),
            format!("{tag}-act-out|ai_recommended"),
        ],
        "activities after scoped confirm; out={acts}"
    );

    let meals = exec_ok(&format!(
        "SELECT day_number || '|' || session_type || '|' || sort_order || '|' || source AS v \
         FROM session_meals WHERE plan_id = {p} AND destination = {d} \
         ORDER BY day_number, session_type, sort_order"
    ));
    assert_eq!(
        meals.column(),
        vec![
            "1|evening|0|ai_recommended".to_string(),
            "1|morning|0|confirmed".to_string(),
            "1|morning|1|confirmed".to_string(),
            "2|morning|0|ai_recommended".to_string(),
        ],
        "session_meals after scoped confirm; out={meals}"
    );

    let routes = exec_ok(&format!(
        "SELECT day_number || '|' || sort_order || '|' || source AS v \
         FROM day_route_segments WHERE plan_id = {p} AND destination = {d} \
         ORDER BY day_number, sort_order"
    ));
    assert_eq!(
        routes.column(),
        vec![
            "1|0|confirmed".to_string(),
            "1|1|confirmed".to_string(),
            "2|0|ai_recommended".to_string(),
        ],
        "day_route_segments after scoped confirm; out={routes}"
    );
    assert_audit(&plan, 1, 1, "confirm-recommendations", 1);

    // ── Scenario 2: zero-scope no-op --day 99 ──
    let out2 = run_cmd(&[
        "confirm-recommendations",
        "--day",
        "99",
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
        stdout2.contains("0 confirmed (nothing matched the scope)"),
        "zero-scope must print the 0-confirmed line; got {stdout2:?}"
    );
    assert_audit(&plan, 1, 1, "confirm-recommendations", 1);

    // ── Scenario 3: --kind meal only — day 1 evening dinner ──
    let out3 = run_cmd(&[
        "confirm-recommendations",
        "--kind",
        "meal",
        "--day",
        "1",
        "--session",
        "evening",
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
        stdout3.contains("✅ 1 confirmed (0 activities, 1 meals, 0 routes)"),
        "meal-only confirm must show the house-style summary; got {stdout3:?}"
    );

    let meals_after_meal_only = exec_ok(&format!(
        "SELECT day_number || '|' || session_type || '|' || sort_order || '|' || source AS v \
         FROM session_meals WHERE plan_id = {p} AND destination = {d} \
         ORDER BY day_number, session_type, sort_order"
    ));
    assert_eq!(
        meals_after_meal_only.column(),
        vec![
            "1|evening|0|confirmed".to_string(),
            "1|morning|0|confirmed".to_string(),
            "1|morning|1|confirmed".to_string(),
            "2|morning|0|ai_recommended".to_string(),
        ],
        "only evening dinner should flip; out={meals_after_meal_only}"
    );

    // activities + routes unchanged from scenario 1
    let acts_unchanged = exec_ok(&format!(
        "SELECT id || '|' || source AS v FROM activities WHERE plan_id = {p} AND destination = {d} ORDER BY id"
    ));
    assert_eq!(
        acts_unchanged.column(),
        acts.column(),
        "meal-only confirm must not touch activities"
    );
    let routes_unchanged = exec_ok(&format!(
        "SELECT day_number || '|' || sort_order || '|' || source AS v \
         FROM day_route_segments WHERE plan_id = {p} AND destination = {d} \
         ORDER BY day_number, sort_order"
    ));
    assert_eq!(
        routes_unchanged.column(),
        routes.column(),
        "meal-only confirm must not touch routes"
    );
    assert_audit(&plan, 2, 2, "confirm-recommendations", 2);

    // ── Scenario 4: route/session contradiction — fail loud, no write ──
    let fail_out = run_cmd_expect_fail(&[
        "confirm-recommendations",
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
    assert_audit(&plan, 2, 2, "confirm-recommendations", 2);
}