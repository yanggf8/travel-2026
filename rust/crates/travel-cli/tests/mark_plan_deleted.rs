//! Real-Turso behavior-LOCK integration test for `mark-plan-deleted` — the soft-delete
//! plan-lifecycle command. Locks its CURRENT DB write surface BEFORE the DAL migration.
//!
//! The command today FUSES the domain write (`plans.deleted_at`) and the audit back-half
//! (`plans.version` bump + the `operation_runs` row) into ONE `UPDATE plans` + one
//! `operation_runs` INSERT. The DAL migration will SPLIT domain (`deleted_at`) from audit
//! (`version` via `record_operation`), so this test asserts the resulting END STATE — NOT
//! any statement count — so it survives the refactor byte-for-byte.
//!
//! Seeds a throwaway `zztest{nanos}` plan at version 0 with NO date_anchor (so the
//! upcoming-trip safety guard passes without `--force`), runs the binary, asserts the
//! soft-delete end state, then runs it AGAIN to lock idempotency. Skips cleanly if Turso
//! creds are absent.

use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{bin, db_exec, is_credless, nanos, seed_plan, teardown_plan, Guard};

static MARK_DELETED_LOCK: Mutex<()> = Mutex::new(());

fn run_mark_deleted(plan: &str) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(["mark-plan-deleted", plan])
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .expect("run mark-plan-deleted");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn mark_plan_deleted_soft_deletes_and_is_idempotent() {
    let _lock = MARK_DELETED_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    if db_exec("SELECT 1 AS n").is_none() {
        eprintln!("skipping mark-plan-deleted test (no Turso creds)");
        return;
    }

    let tag = nanos();
    let plan = format!("zztest{tag}");
    let dest = format!("zztest_dest_{tag}");

    teardown_plan(&plan, &dest);
    // Guard runs teardown on return AND on panic (so a failing assert can't leak rows).
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });

    seed_plan(&plan, &dest, 0);

    // --- 1. first run: soft-delete succeeds ---
    let (ok, stdout, stderr) = run_mark_deleted(&plan);
    if !ok && is_credless(&stderr) {
        eprintln!(
            "skipping mark-plan-deleted test (no Turso creds): {}",
            stderr.trim()
        );
        return;
    }
    assert!(
        ok,
        "mark-plan-deleted should succeed; stdout={stdout} stderr={stderr}"
    );
    assert!(
        stdout.contains(&format!("marked {plan} deleted (soft)")),
        "stdout should confirm the soft-delete; stdout={stdout}"
    );

    // END STATE: plans.deleted_at IS NOT NULL.
    let deleted = db_exec(&format!(
        "SELECT CASE WHEN deleted_at IS NOT NULL THEN 'set' ELSE 'null' END AS v \
         FROM plans WHERE plan_id = '{plan}'"
    ))
    .expect("creds");
    assert_eq!(
        deleted.scalar().as_deref(),
        Some("set"),
        "plans.deleted_at should be set (soft-deleted); out={deleted}"
    );

    // END STATE: plans.version bumped 0 -> 1.
    let version = db_exec(&format!(
        "SELECT version AS v FROM plans WHERE plan_id = '{plan}'"
    ))
    .expect("creds");
    assert_eq!(
        version.scalar().as_deref(),
        Some("1"),
        "plans.version should bump by one; out={version}"
    );

    // END STATE: exactly one mark-plan-deleted operation_runs row, completed, 0 -> 1.
    let op_count = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs \
         WHERE plan_id = '{plan}' AND command_type = 'mark-plan-deleted'"
    ))
    .expect("creds");
    assert_eq!(
        op_count.scalar().as_deref(),
        Some("1"),
        "exactly one mark-plan-deleted operation_run; out={op_count}"
    );

    let op_row = db_exec(&format!(
        "SELECT status || '|' || version_before || '|' || version_after AS v \
         FROM operation_runs \
         WHERE plan_id = '{plan}' AND command_type = 'mark-plan-deleted'"
    ))
    .expect("creds");
    assert_eq!(
        op_row.scalar().as_deref(),
        Some("completed|0|1"),
        "operation_run should record status=completed version_before=0 version_after=1; out={op_row}"
    );

    // --- 2. second run: idempotent (no change, no 2nd audit row) ---
    let (ok2, stdout2, stderr2) = run_mark_deleted(&plan);
    assert!(
        ok2,
        "re-run mark-plan-deleted should exit 0 (idempotent); stdout={stdout2} stderr={stderr2}"
    );
    assert!(
        stdout2.contains(&format!("{plan} is already marked deleted (soft)")),
        "re-run stdout should report already deleted; stdout={stdout2}"
    );

    // Still exactly one operation_runs row (no second audit row written).
    let op_count2 = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs \
         WHERE plan_id = '{plan}' AND command_type = 'mark-plan-deleted'"
    ))
    .expect("creds");
    assert_eq!(
        op_count2.scalar().as_deref(),
        Some("1"),
        "re-run must NOT write a second operation_run; out={op_count2}"
    );

    // version unchanged (still 1).
    let version2 = db_exec(&format!(
        "SELECT version AS v FROM plans WHERE plan_id = '{plan}'"
    ))
    .expect("creds");
    assert_eq!(
        version2.scalar().as_deref(),
        Some("1"),
        "re-run must NOT bump plans.version again; out={version2}"
    );
}