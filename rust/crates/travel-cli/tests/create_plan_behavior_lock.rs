//! Real-Turso behavior LOCK for `create-plan` — fast-path plan seed
//! (plans + metadata + date_anchors + process ladder) via `create_plan_seed`.

use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{bin, db_exec, db_exec_teardown, is_credless, nanos, teardown_plan, Guard};

static LOCK: Mutex<()> = Mutex::new(());

fn sql_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn exec_ok(sql: &str) -> common::Rows {
    db_exec(sql).unwrap_or_else(|| panic!("db exec skipped unexpectedly for SQL: {sql}"))
}

fn run_cmd(args: &[&str]) -> Option<(String, String)> {
    let out = Command::new(bin())
        .args(args)
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() && is_credless(&stderr) {
        eprintln!("skipping create-plan lock mid-test: {}", stderr.trim());
        return None;
    }
    assert!(
        out.status.success(),
        "travel {args:?} should succeed; stdout={stdout} stderr={stderr}"
    );
    Some((stdout, stderr))
}

fn run_cmd_expect_fail(args: &[&str]) -> Option<(String, String)> {
    let out = Command::new(bin())
        .args(args)
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if is_credless(&stderr) {
        eprintln!("skipping create-plan lock mid-test: {}", stderr.trim());
        return None;
    }
    assert!(
        !out.status.success(),
        "travel {args:?} should FAIL but succeeded; stdout={stdout} stderr={stderr}"
    );
    Some((stdout, stderr))
}

fn teardown_all(plan: &str, dest: &str, extra_plans: &[&str]) {
    teardown_plan(plan, dest);
    for p in extra_plans {
        teardown_plan(p, dest);
    }
    let d = sql_lit(dest);
    let _ = db_exec_teardown(&format!("DELETE FROM destination_config WHERE slug = {d}"));
}

#[test]
fn create_plan_behavior_lock() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());

    if db_exec("SELECT 1 AS n").is_none() {
        eprintln!("skipping create-plan lock (no Turso creds)");
        return;
    }

    let tag = nanos();
    let plan = format!("zztest-createplan-{tag}");
    let dest = format!("zzcreateplan_{tag}");
    let plan_unreg = format!("zztest-createplan-x-{tag}");
    let plan_bad = format!("zztest-createplan-bad-{tag}");

    teardown_all(&plan, &dest, &[&plan_unreg, &plan_bad]);
    let _g = Guard::new({
        let (plan, dest, plan_unreg, plan_bad) = (
            plan.clone(),
            dest.clone(),
            plan_unreg.clone(),
            plan_bad.clone(),
        );
        move || teardown_all(&plan, &dest, &[&plan_unreg, &plan_bad])
    });

    let d = sql_lit(&dest);
    if db_exec(&format!(
        "INSERT INTO destination_config (slug, display_name, timezone, currency, origin) \
         VALUES ({d}, 'Zz Test City', 'Asia/Tokyo', 'JPY', 'taiwan')"
    ))
    .is_none()
    {
        return;
    }

    // ── Scenario 1: happy path ──
    let out = run_cmd(&[
        "create-plan",
        &plan,
        "--dest",
        &dest,
        "--start",
        "2026-06-18",
        "--end",
        "2026-06-24",
        "--airport",
        "NGO",
        "--region",
        "japan",
    ]);
    if out.is_none() {
        return;
    }
    let (stdout, _) = out.unwrap();
    assert!(
        stdout.contains(&format!("✅ Created plan {plan}")),
        "stdout must show create summary; got {stdout:?}"
    );

    let p = sql_lit(&plan);
    let count = exec_ok(&format!("SELECT COUNT(*) AS n FROM plans WHERE plan_id = {p}"));
    assert_eq!(count.scalar().as_deref(), Some("1"), "plans row count");

    let version = exec_ok(&format!("SELECT version AS v FROM plans WHERE plan_id = {p}"));
    assert_eq!(version.scalar().as_deref(), Some("1"), "plans.version");

    let active = exec_ok(&format!(
        "SELECT active_destination AS v FROM plan_metadata WHERE plan_id = {p}"
    ));
    assert_eq!(active.scalar().as_deref(), Some(dest.as_str()), "active_destination");

    let anchor = exec_ok(&format!(
        "SELECT start_date || '|' || end_date || '|' || days AS v FROM date_anchors WHERE plan_id = {p}"
    ));
    assert_eq!(
        anchor.scalar().as_deref(),
        Some("2026-06-18|2026-06-24|7"),
        "date_anchors"
    );

    let processes = exec_ok(&format!(
        "SELECT process_id || '=' || status AS v FROM process_statuses WHERE plan_id = {p} ORDER BY process_id"
    ));
    let proc_set: std::collections::BTreeSet<String> =
        processes.column().into_iter().collect();
    assert!(
        proc_set.contains("process_1_date_anchor=confirmed"),
        "process_1 must be confirmed; got {proc_set:?}"
    );
    assert!(
        proc_set.contains("process_2_destination=confirmed"),
        "process_2 must be confirmed; got {proc_set:?}"
    );
    assert!(
        proc_set.contains("process_5_daily_itinerary=pending"),
        "process_5 must be pending; got {proc_set:?}"
    );

    let op = exec_ok(&format!(
        "SELECT command_type || '|' || version_before || '>' || version_after || '|' || status AS v \
         FROM operation_runs WHERE plan_id = {p}"
    ));
    assert_eq!(
        op.column(),
        vec!["create-plan|0>1|completed".to_string()],
        "operation_runs audit row"
    );

    // ── Scenario 2: duplicate plan_id ──
    let dup = run_cmd_expect_fail(&[
        "create-plan",
        &plan,
        "--dest",
        &dest,
        "--start",
        "2026-06-18",
        "--end",
        "2026-06-24",
        "--airport",
        "NGO",
    ]);
    if dup.is_none() {
        return;
    }
    let (_, stderr_dup) = dup.unwrap();
    assert!(
        stderr_dup.contains("Plan already exists"),
        "dup must fail loud; stderr={stderr_dup:?}"
    );

    let version_after_dup =
        exec_ok(&format!("SELECT version AS v FROM plans WHERE plan_id = {p}"));
    assert_eq!(
        version_after_dup.scalar().as_deref(),
        Some("1"),
        "version must stay 1 after dup attempt"
    );

    let op_count = exec_ok(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id = {p}"
    ));
    assert_eq!(
        op_count.scalar().as_deref(),
        Some("1"),
        "operation_runs count must stay 1 after dup attempt"
    );

    // ── Scenario 3: unregistered destination ──
    let unreg_dest = format!("zz_nope_{tag}");
    let fail_unreg = run_cmd_expect_fail(&[
        "create-plan",
        &plan_unreg,
        "--dest",
        &unreg_dest,
        "--start",
        "2026-06-18",
        "--end",
        "2026-06-24",
        "--airport",
        "NGO",
    ]);
    if fail_unreg.is_none() {
        return;
    }
    let (_, stderr_unreg) = fail_unreg.unwrap();
    assert!(
        stderr_unreg.contains("Destination config not found"),
        "unregistered dest must fail loud; stderr={stderr_unreg:?}"
    );

    let p_unreg = sql_lit(&plan_unreg);
    let unreg_count =
        exec_ok(&format!("SELECT COUNT(*) AS n FROM plans WHERE plan_id = {p_unreg}"));
    assert_eq!(
        unreg_count.scalar().as_deref(),
        Some("0"),
        "unregistered dest must not create a plans row"
    );

    // ── Scenario 4: bad dates (end < start) ──
    let fail_dates = run_cmd_expect_fail(&[
        "create-plan",
        &plan_bad,
        "--dest",
        &dest,
        "--start",
        "2026-06-24",
        "--end",
        "2026-06-18",
        "--airport",
        "NGO",
    ]);
    if fail_dates.is_none() {
        return;
    }
    let (_, stderr_dates) = fail_dates.unwrap();
    assert!(
        stderr_dates.contains("cannot be after end date"),
        "bad dates must fail loud; stderr={stderr_dates:?}"
    );

    let p_bad = sql_lit(&plan_bad);
    let bad_count = exec_ok(&format!("SELECT COUNT(*) AS n FROM plans WHERE plan_id = {p_bad}"));
    assert_eq!(
        bad_count.scalar().as_deref(),
        Some("0"),
        "bad dates must not create a plans row"
    );
}