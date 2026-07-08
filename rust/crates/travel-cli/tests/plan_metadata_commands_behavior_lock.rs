//! Real-Turso behavior LOCK for `set-plan-name` and `set-active-destination` —
//! plan-scoped metadata mutations with audit (version + operation_runs) but no plan_events.

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
        eprintln!("skipping plan-metadata lock mid-test: {}", stderr.trim());
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
        eprintln!("skipping plan-metadata lock mid-test: {}", stderr.trim());
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

fn insert_plan_destination(plan: &str, slug: &str, display_name: &str) {
    let p = sql_lit(plan);
    let s = sql_lit(slug);
    let n = sql_lit(display_name);
    // OR REPLACE: seed_plan now inserts a default plan_destinations row for the
    // active dest, so this override must replace it (this test controls display_name).
    exec_ok(&format!(
        "INSERT OR REPLACE INTO plan_destinations (plan_id, slug, display_name, status, created_at, updated_at) \
         VALUES ({p}, {s}, {n}, 'active', '2020-01-01 00:00:00', '2020-01-01 00:00:00')"
    ));
}

fn assert_version(plan: &str, expected: i64) {
    let p = sql_lit(plan);
    let version = exec_ok(&format!("SELECT version AS v FROM plans WHERE plan_id = {p}"));
    assert_eq!(
        version.scalar().as_deref(),
        Some(expected.to_string().as_str()),
        "plans.version for {plan}"
    );
}

fn assert_op_runs(plan: &str, command_type: &str, expected: i64) {
    let p = sql_lit(plan);
    let cmd = sql_lit(command_type);
    let count = exec_ok(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id = {p} AND command_type = {cmd}"
    ));
    assert_eq!(
        count.scalar().as_deref(),
        Some(expected.to_string().as_str()),
        "operation_runs count for {command_type} on {plan}"
    );
}

fn assert_plan_events_zero(plan: &str) {
    let p = sql_lit(plan);
    let count = exec_ok(&format!(
        "SELECT COUNT(*) AS n FROM plan_events WHERE plan_id = {p}"
    ));
    assert_eq!(
        count.scalar().as_deref(),
        Some("0"),
        "plan_events must stay 0 for plan-level metadata mutations on {plan}"
    );
}

#[test]
fn plan_metadata_commands_write_surface_is_locked() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());

    if db_exec("SELECT 1 AS n").is_none() {
        eprintln!("skipping plan-metadata commands lock (no Turso creds)");
        return;
    }

    let tag = nanos();

    // ── Scenario A: set-plan-name rename (single-dest) ──
    let plan_a = format!("zztest-planname-{tag}");
    let dest_a = format!("zzplanname_{tag}");

    let _g = Guard::new({
        let plans = vec![
            (plan_a.clone(), dest_a.clone()),
            (format!("zztest-actdest-{tag}"), format!("zzactdest_a_{tag}")),
            (format!("zztest-actdest-inv-{tag}"), format!("zzactdest_inv_{tag}")),
            (format!("zztest-multidest-{tag}"), format!("zzmultidest_a_{tag}")),
        ];
        move || {
            for (plan, dest) in &plans {
                teardown(plan, dest);
            }
        }
    });

    teardown(&plan_a, &dest_a);
    seed_plan(&plan_a, &dest_a, 10);
    insert_plan_destination(&plan_a, &dest_a, "Old Name");

    let out_a = run_cmd(&[
        "set-plan-name",
        "New Drill Name",
        "--plan-id",
        &plan_a,
    ]);
    if out_a.is_none() {
        return;
    }
    let (stdout_a, _) = out_a.unwrap();
    assert!(
        stdout_a.contains(&format!("✅ Renamed plan {plan_a} → \"New Drill Name\"")),
        "stdout must show rename confirmation; got {stdout_a:?}"
    );

    let p_a = sql_lit(&plan_a);
    let d_a = sql_lit(&dest_a);
    let display = exec_ok(&format!(
        "SELECT display_name AS v FROM plan_destinations WHERE plan_id = {p_a} AND slug = {d_a}"
    ));
    assert_eq!(
        display.scalar().as_deref(),
        Some("New Drill Name"),
        "display_name after set-plan-name"
    );
    assert_version(&plan_a, 11);
    assert_op_runs(&plan_a, "set-plan-name", 1);
    assert_plan_events_zero(&plan_a);

    // ── Scenario B: set-active-destination valid ──
    let plan_b = format!("zztest-actdest-{tag}");
    let dest_b_a = format!("zzactdest_a_{tag}");
    let dest_b_b = format!("zzactdest_b_{tag}");

    teardown(&plan_b, &dest_b_a);
    seed_plan(&plan_b, &dest_b_a, 20);
    insert_plan_destination(&plan_b, &dest_b_a, "Dest A");
    insert_plan_destination(&plan_b, &dest_b_b, "Dest B");

    let out_b = run_cmd(&[
        "set-active-destination",
        &dest_b_b,
        "--plan-id",
        &plan_b,
    ]);
    if out_b.is_none() {
        return;
    }
    let (stdout_b, _) = out_b.unwrap();
    assert!(
        stdout_b.contains(&format!("✅ Active destination for {plan_b} → {dest_b_b}")),
        "stdout must show active-destination confirmation; got {stdout_b:?}"
    );

    let p_b = sql_lit(&plan_b);
    let active = exec_ok(&format!(
        "SELECT active_destination AS v FROM plan_metadata WHERE plan_id = {p_b}"
    ));
    assert_eq!(
        active.scalar().as_deref(),
        Some(dest_b_b.as_str()),
        "active_destination after set-active-destination"
    );
    assert_version(&plan_b, 21);
    assert_op_runs(&plan_b, "set-active-destination", 1);
    assert_plan_events_zero(&plan_b);

    // ── Scenario C: set-active-destination invalid slug ──
    let plan_c = format!("zztest-actdest-inv-{tag}");
    let dest_c = format!("zzactdest_inv_{tag}");

    teardown(&plan_c, &dest_c);
    seed_plan(&plan_c, &dest_c, 30);
    insert_plan_destination(&plan_c, &dest_c, "Only Dest");

    let fail_c = run_cmd_expect_fail(&[
        "set-active-destination",
        "missing_dest",
        "--plan-id",
        &plan_c,
    ]);
    if fail_c.is_none() {
        return;
    }
    let (_, stderr_c) = fail_c.unwrap();
    assert!(
        stderr_c.contains(&format!(
            "destination missing_dest is not a destination of plan {plan_c}"
        )),
        "stderr must contain slug validation message; got {stderr_c:?}"
    );

    let p_c = sql_lit(&plan_c);
    let active_c = exec_ok(&format!(
        "SELECT active_destination AS v FROM plan_metadata WHERE plan_id = {p_c}"
    ));
    assert_eq!(
        active_c.scalar().as_deref(),
        Some(dest_c.as_str()),
        "active_destination must be unchanged after invalid slug"
    );
    assert_version(&plan_c, 30);
    assert_op_runs(&plan_c, "set-active-destination", 0);

    // ── Scenario D: multi-dest rename without --dest ──
    let plan_d = format!("zztest-multidest-{tag}");
    let dest_d_a = format!("zzmultidest_a_{tag}");
    let dest_d_b = format!("zzmultidest_b_{tag}");

    teardown(&plan_d, &dest_d_a);
    seed_plan(&plan_d, &dest_d_a, 40);
    insert_plan_destination(&plan_d, &dest_d_a, "Old Name A");
    insert_plan_destination(&plan_d, &dest_d_b, "Old Name B");

    let fail_d = run_cmd_expect_fail(&[
        "set-plan-name",
        "Ambiguous Name",
        "--plan-id",
        &plan_d,
    ]);
    if fail_d.is_none() {
        return;
    }
    let (_, stderr_d) = fail_d.unwrap();
    assert!(
        stderr_d.contains(&format!(
            "plan {plan_d} has multiple destinations; pass --dest <slug>"
        )),
        "stderr must contain multi-dest disambiguation message; got {stderr_d:?}"
    );

    let p_d = sql_lit(&plan_d);
    let names = exec_ok(&format!(
        "SELECT slug || '|' || display_name AS v FROM plan_destinations \
         WHERE plan_id = {p_d} ORDER BY slug"
    ));
    assert_eq!(
        names.column(),
        vec![
            format!("{dest_d_a}|Old Name A"),
            format!("{dest_d_b}|Old Name B"),
        ],
        "display_names must be unchanged after ambiguous rename attempt"
    );
    assert_version(&plan_d, 40);
}