//! Real-Turso behavior LOCK for `swap-days` full session repointing.
//!
//! This pins the current un-migrated command behavior: day-level theme fields
//! are swapped, date/day_type stay with each day_number, all four
//! session-scoped tables are re-pointed dayA <-> dayB via the command's TMP
//! dance, both day rows are touched, and the hand-rolled audit writes one
//! operation run plus one plan version bump.

use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{bin, db_exec, db_exec_teardown, is_credless, nanos, seed_plan, teardown_plan, Guard};

static LOCK: Mutex<()> = Mutex::new(());

fn sql_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn exec_ok(sql: &str) -> common::Rows {
    db_exec(sql).unwrap_or_else(|| panic!("db exec unexpectedly skipped after probe; sql={sql}"))
}

fn teardown(plan: &str, dest: &str) {
    let p = sql_lit(plan);
    let d = sql_lit(dest);
    let _ = db_exec_teardown(&format!(
        "DELETE FROM activity_tags \
           WHERE activity_id IN (SELECT id FROM activities WHERE plan_id = {p} AND destination = {d});"
    ));
    teardown_plan(plan, dest);
}

fn seed_plan_days_and_sessions(plan: &str, dest: &str) {
    let p = sql_lit(plan);
    let d = sql_lit(dest);
    seed_plan(plan, dest, 41);
    exec_ok(&format!(
        "INSERT INTO days \
           (plan_id, destination, day_number, date, theme, theme_zh, day_type, status, updated_at) \
           VALUES \
           ({p}, {d}, 1, '2026-09-01', 'A_THEME', 'A_THEME_ZH', 'arrival', 'draft', '2001-01-01 00:00:00'), \
           ({p}, {d}, 2, '2026-09-02', 'B_THEME', 'B_THEME_ZH', 'departure', 'planned', '2001-01-02 00:00:00');"
    ));

    for (day, label, morning_start, evening_start) in
        [(1, "A", "08:00", "18:00"), (2, "B", "09:00", "19:00")]
    {
        exec_ok(&format!(
            "INSERT INTO timesofday \
                (plan_id, destination, day_number, session_type, focus, transit_notes, \
                 booking_notes, time_range_start, time_range_end, focus_zh, transit_notes_zh, updated_at) \
             VALUES \
                ({p}, {d}, {day}, 'morning', '{label}_tod_morning', '{label}_transit_morning', \
                 '{label}_booking_morning', '{morning_start}', '11:00', '{label}_focus_zh_morning', \
                 '{label}_transit_zh_morning', '2001-01-01 00:00:00'), \
                ({p}, {d}, {day}, 'evening', '{label}_tod_evening', '{label}_transit_evening', \
                 '{label}_booking_evening', '{evening_start}', '21:00', '{label}_focus_zh_evening', \
                 '{label}_transit_zh_evening', '2001-01-01 00:00:00'); \
             INSERT INTO activities \
                (id, plan_id, destination, day_number, session_type, sort_order, title, area, \
                 nearest_station, duration_min, booking_required, booking_url, booking_status, \
                 booking_ref, book_by, start_time, end_time, is_fixed_time, cost_estimate, \
                 notes, priority, updated_at) \
             VALUES \
                ('{plan}_act_{label}_morning', {p}, {d}, {day}, 'morning', 10, '{label}_activity_morning', \
                 '{label}_area_morning', '{label}_station_morning', 60, 1, 'https://example.test/{label}/morning', \
                 'pending', '{label}-REF-M', '2026-08-01', '{morning_start}', '10:00', 1, 1000, \
                 '{label}_notes_morning', 'must', '2001-01-01 00:00:00'), \
                ('{plan}_act_{label}_evening', {p}, {d}, {day}, 'evening', 20, '{label}_activity_evening', \
                 '{label}_area_evening', '{label}_station_evening', 90, 0, NULL, \
                 'not_required', NULL, NULL, '{evening_start}', '20:30', 0, 2000, \
                 '{label}_notes_evening', 'optional', '2001-01-01 00:00:00'); \
             INSERT INTO activity_tags (activity_id, tag, updated_at) VALUES \
                ('{plan}_act_{label}_morning', '{label}_tag_morning', '2001-01-01 00:00:00'), \
                ('{plan}_act_{label}_evening', '{label}_tag_evening', '2001-01-01 00:00:00'); \
             INSERT INTO session_meals \
                (plan_id, destination, day_number, session_type, sort_order, meal, updated_at) \
             VALUES \
                ({p}, {d}, {day}, 'morning', 1, '{label}_meal_morning', '2001-01-01 00:00:00'), \
                ({p}, {d}, {day}, 'evening', 2, '{label}_meal_evening', '2001-01-01 00:00:00'); \
             INSERT INTO session_activities_zh \
                (plan_id, destination, day_number, session_type, sort_order, activity) \
             VALUES \
                ({p}, {d}, {day}, 'morning', 1, '{label}_activity_zh_morning'), \
                ({p}, {d}, {day}, 'evening', 2, '{label}_activity_zh_evening');"
        ));
    }
}

fn run_swap_days(plan: &str, dest: &str) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(["swap-days", "1", "2", "--plan-id", plan, "--dest", dest])
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .expect("run travel swap-days");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn assert_values(sql: &str, expected: &[&str], context: &str) {
    let out = exec_ok(sql);
    let values = out.column();
    let expected = expected.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(values, expected, "{context}; out={out}");
}

#[test]
fn swap_days_repoints_every_session_scoped_table_and_audits_once() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());

    if db_exec("SELECT 1 AS n").is_none() {
        return;
    }

    let tag = nanos();
    let plan = format!("zztest-{tag}");
    let dest = format!("zztest_{tag}");
    let p = sql_lit(&plan);
    let d = sql_lit(&dest);
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown(&plan, &dest)
    });

    teardown(&plan, &dest);
    seed_plan_days_and_sessions(&plan, &dest);

    let before = exec_ok(&format!(
        "SELECT version AS v FROM plans WHERE plan_id = {p}"
    ));
    assert_eq!(before.scalar().as_deref(), Some("41"), "seed sanity");

    let (ok, stdout, stderr) = run_swap_days(&plan, &dest);
    if !ok && is_credless(&stderr) {
        eprintln!(
            "skipping swap-days repoint lock mid-test: {}",
            stderr.trim()
        );
        return;
    }
    assert!(
        ok,
        "swap-days should succeed; stdout={stdout}\nstderr={stderr}"
    );

    let day_rows = exec_ok(&format!(
        "SELECT day_number || '|' || date || '|' || day_type || '|' || status || '|' || \
                COALESCE(theme, '<NULL>') || '|' || COALESCE(theme_zh, '<NULL>') AS rowval \
         FROM days WHERE plan_id = {p} AND destination = {d} ORDER BY day_number"
    ));
    assert_eq!(
        day_rows.column(),
        vec![
            "1|2026-09-01|arrival|draft|B_THEME|B_THEME_ZH".to_string(),
            "2|2026-09-02|departure|planned|A_THEME|A_THEME_ZH".to_string(),
        ],
        "themes/theme_zh swap while date/day_type/status stay on their original day rows; out={day_rows}"
    );

    let old_timestamps = exec_ok(&format!(
        "SELECT COUNT(*) AS n \
         FROM days WHERE plan_id = {p} AND destination = {d} \
           AND ((day_number = 1 AND updated_at = '2001-01-01 00:00:00') \
             OR (day_number = 2 AND updated_at = '2001-01-02 00:00:00'))"
    ));
    assert_eq!(
        old_timestamps.scalar().as_deref(),
        Some("0"),
        "current swap-days touches both day rows after the theme-only UPDATE; out={old_timestamps}"
    );

    assert_values(
        &format!(
            "SELECT day_number || ':' || COUNT(*) AS rowval \
             FROM timesofday WHERE plan_id = {p} AND destination = {d} \
             GROUP BY day_number ORDER BY day_number"
        ),
        &["1:2", "2:2"],
        "timesofday row counts should be swapped without duplication/drop",
    );
    assert_values(
        &format!(
            "SELECT day_number || '|' || session_type || '|' || focus || '|' || time_range_start AS rowval \
             FROM timesofday WHERE plan_id = {p} AND destination = {d} \
             ORDER BY day_number, CASE session_type WHEN 'morning' THEN 0 ELSE 1 END"
        ),
        &[
            "1|morning|B_tod_morning|09:00",
            "1|evening|B_tod_evening|19:00",
            "2|morning|A_tod_morning|08:00",
            "2|evening|A_tod_evening|18:00",
        ],
        "timesofday ownership should be fully re-pointed",
    );

    assert_values(
        &format!(
            "SELECT day_number || ':' || COUNT(*) AS rowval \
             FROM activities WHERE plan_id = {p} AND destination = {d} \
             GROUP BY day_number ORDER BY day_number"
        ),
        &["1:2", "2:2"],
        "activities row counts should be swapped without duplication/drop",
    );
    assert_values(
        &format!(
            "SELECT day_number || '|' || id || '|' || title || '|' || booking_status || '|' || priority AS rowval \
             FROM activities WHERE plan_id = {p} AND destination = {d} \
             ORDER BY day_number, CASE session_type WHEN 'morning' THEN 0 ELSE 1 END"
        ),
        &[
            &format!("1|{plan}_act_B_morning|B_activity_morning|pending|must"),
            &format!("1|{plan}_act_B_evening|B_activity_evening|not_required|optional"),
            &format!("2|{plan}_act_A_morning|A_activity_morning|pending|must"),
            &format!("2|{plan}_act_A_evening|A_activity_evening|not_required|optional"),
        ],
        "activities ownership should be fully re-pointed",
    );

    assert_values(
        &format!(
            "SELECT day_number || ':' || COUNT(*) AS rowval \
             FROM session_meals WHERE plan_id = {p} AND destination = {d} \
             GROUP BY day_number ORDER BY day_number"
        ),
        &["1:2", "2:2"],
        "session_meals row counts should be swapped without duplication/drop",
    );
    assert_values(
        &format!(
            "SELECT day_number || '|' || session_type || '|' || sort_order || '|' || meal AS rowval \
             FROM session_meals WHERE plan_id = {p} AND destination = {d} \
             ORDER BY day_number, sort_order"
        ),
        &[
            "1|morning|1|B_meal_morning",
            "1|evening|2|B_meal_evening",
            "2|morning|1|A_meal_morning",
            "2|evening|2|A_meal_evening",
        ],
        "session_meals ownership should be fully re-pointed",
    );

    assert_values(
        &format!(
            "SELECT day_number || ':' || COUNT(*) AS rowval \
             FROM session_activities_zh WHERE plan_id = {p} AND destination = {d} \
             GROUP BY day_number ORDER BY day_number"
        ),
        &["1:2", "2:2"],
        "session_activities_zh row counts should be swapped without duplication/drop",
    );
    assert_values(
        &format!(
            "SELECT day_number || '|' || session_type || '|' || sort_order || '|' || activity AS rowval \
             FROM session_activities_zh WHERE plan_id = {p} AND destination = {d} \
             ORDER BY day_number, sort_order"
        ),
        &[
            "1|morning|1|B_activity_zh_morning",
            "1|evening|2|B_activity_zh_evening",
            "2|morning|1|A_activity_zh_morning",
            "2|evening|2|A_activity_zh_evening",
        ],
        "session_activities_zh ownership should be fully re-pointed",
    );

    for table in [
        "timesofday",
        "activities",
        "session_meals",
        "session_activities_zh",
    ] {
        let out = exec_ok(&format!(
            "SELECT COUNT(*) AS n FROM {table} \
             WHERE plan_id = {p} AND destination = {d} AND day_number = -999999"
        ));
        assert_eq!(
            out.scalar().as_deref(),
            Some("0"),
            "{table} must not retain TMP rows; out={out}"
        );
    }

    let op_count = exec_ok(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id = {p}"
    ));
    assert_eq!(
        op_count.scalar().as_deref(),
        Some("1"),
        "exactly one operation_runs row should be written; out={op_count}"
    );
    let op = exec_ok(&format!(
        "SELECT command_type || '|' || status || '|' || version_before || '>' || \
                COALESCE(version_after, -1) AS rowval \
         FROM operation_runs WHERE plan_id = {p}"
    ));
    assert_eq!(
        op.scalar().as_deref(),
        Some("swap-days|completed|41>42"),
        "operation_runs should capture the swap-days audit row; out={op}"
    );
    let version = exec_ok(&format!(
        "SELECT version AS v FROM plans WHERE plan_id = {p}"
    ));
    assert_eq!(
        version.scalar().as_deref(),
        Some("42"),
        "plans.version should bump by one; out={version}"
    );
}