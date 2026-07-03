//! Real-Turso integration tests for the soft-delete fail-proofing feature:
//!   - `mark-plan-deleted <plan>` sets plans.deleted_at and the plan stops
//!     appearing in `travel plans`.
//!   - `mark-plan-deleted <nonexistent>` exits non-zero.
//!   - `db exec` on a 0-row DELETE prints `0 row(s) affected` + the ⚠ warning.
//!   - `db cleanup-deleted` (dry-run) reports counts without deleting; a real
//!     `db cleanup-deleted --confirm` then wipes the plan's rows.
//!
//! Harness mirrors tests/set_mutation_bugs.rs: seed a throwaway plan, run the
//! binary, SELECT to assert, tear down. Skips cleanly when Turso creds are
//! absent. NEVER touches okinawa-2026 / tokyo-2026 / kyoto-2026 — all plan ids
//! are nanos-tagged throwaways.

use std::process::Command;

mod common;
use common::{bin, db_exec, nanos, seed_plan, teardown_plan, Guard};

/// Seed only plans + plan_metadata for a throwaway plan (no date anchor → a
/// junk-style plan that flags freely). Returns false on a credless skip.
fn seed_bare(plan_id: &str, dest: &str) -> bool {
    if db_exec("SELECT 1 AS n").is_none() {
        return false;
    }
    seed_plan(plan_id, dest, 0);
    true
}

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

/// SELECT one scalar via db exec, parsing the `<col>: <value>` plain-text line.
fn scalar(sql: &str, col: &str) -> Option<String> {
    let stdout = db_exec(sql)?.raw().to_string();
    let prefix = format!("{col}: ");
    Some(
        stdout
            .lines()
            .find_map(|l| l.strip_prefix(&prefix))
            .map(|s| s.trim().to_string())
            .unwrap_or_default(),
    )
}

// ── 1. mark-plan-deleted sets deleted_at; plan drops from `travel plans` ──────
#[test]
fn mark_plan_deleted_soft_deletes_and_hides() {
    let tag = nanos();
    let plan_id = format!("test-softdel-{tag}");
    let dest = format!("softdel_{tag}");
    if !seed_bare(&plan_id, &dest) {
        return;
    }
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown_plan(&plan_id, &dest)
    });

    // Pre-condition: plan lists.
    let (_, plans_before, _) = run_cmd(&["plans"]);
    assert!(
        plans_before.contains(&plan_id),
        "freshly-seeded plan should list before soft-delete; got:\n{plans_before}"
    );

    let (ok, stdout, stderr) = run_cmd(&["mark-plan-deleted", &plan_id]);
    assert!(ok, "mark-plan-deleted should succeed; stdout={stdout} stderr={stderr}");
    assert!(
        stdout.contains("marked") && stdout.contains("deleted (soft)"),
        "expected soft-delete confirmation; got: {stdout}"
    );

    // deleted_at is now set.
    let deleted_at = scalar(
        &format!("SELECT deleted_at AS d FROM plans WHERE plan_id = '{plan_id}'"),
        "d",
    );
    let was_set = deleted_at.map(|s| !s.is_empty()).unwrap_or(false);

    // The plan no longer lists.
    let (_, plans_after, _) = run_cmd(&["plans"]);
    let still_listed = plans_after.contains(&plan_id);

    // Idempotent re-run reports "already".
    let (ok2, stdout2, _) = run_cmd(&["mark-plan-deleted", &plan_id]);

    assert!(was_set, "plans.deleted_at must be set after mark-plan-deleted");
    assert!(
        !still_listed,
        "soft-deleted plan must NOT appear in `travel plans`; got:\n{plans_after}"
    );
    assert!(ok2, "idempotent re-run should exit 0");
    assert!(
        stdout2.contains("already marked deleted"),
        "idempotent re-run should report 'already'; got: {stdout2}"
    );
}

// ── 2. mark-plan-deleted on a nonexistent plan exits non-zero ─────────────────
#[test]
fn mark_plan_deleted_nonexistent_fails_loud() {
    // Use a credless-skip probe first (cheap SELECT) so we don't false-fail.
    if db_exec("SELECT 1 AS n").is_none() {
        return;
    }
    let plan_id = format!("test-softdel-nope-{}", nanos());
    let (ok, stdout, stderr) = run_cmd(&["mark-plan-deleted", &plan_id]);
    assert!(
        !ok,
        "marking a nonexistent plan must exit non-zero; stdout={stdout} stderr={stderr}"
    );
    assert!(
        stderr.contains("not found"),
        "error must explain the plan is missing; got: {stderr}"
    );
}

// ── 3. db exec 0-row DELETE prints count + ⚠ warning ──────────────────────────
#[test]
fn db_exec_zero_row_delete_warns() {
    if db_exec("SELECT 1 AS n").is_none() {
        return;
    }
    let nothing = format!("no-such-plan-{}", nanos());
    let rows =
        db_exec(&format!("DELETE FROM plan_metadata WHERE plan_id = '{nothing}'"))
            .expect("db exec should succeed (0-row delete is not an error)");
    let stdout = rows.raw().to_string();
    let stderr = rows.stderr.clone();
    assert!(
        stdout.contains("0 row(s) affected"),
        "0-row mutation must print the count; stdout={stdout}"
    );
    assert!(
        stderr.contains("no rows matched") && stderr.contains("nothing changed"),
        "0-row mutation must warn on stderr; stderr={stderr}"
    );
}

// ── 4. db cleanup-deleted: dry-run reports, --confirm wipes ───────────────────
#[test]
fn db_cleanup_deleted_dry_run_then_wipe() {
    let tag = nanos();
    let plan_id = format!("test-softdel-cleanup-{tag}");
    let dest = format!("softdelcl_{tag}");
    if !seed_bare(&plan_id, &dest) {
        return;
    }
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown_plan(&plan_id, &dest)
    });

    // Seed a couple of plan-scoped child rows so there's something to count.
    let _ = db_exec(&format!(
        "INSERT INTO date_anchors (plan_id, destination, start_date, end_date, days) \
         VALUES ('{plan_id}', '{dest}', '2020-01-01', '2020-01-03', 3)"
    ));

    // Flag it.
    let (ok, _, stderr) = run_cmd(&["mark-plan-deleted", &plan_id]);
    assert!(ok, "mark-plan-deleted should succeed; stderr={stderr}");

    // Dry run (default): reports, deletes nothing.
    let (ok_dry, dry_out, dry_err) = run_cmd(&["db", "cleanup-deleted"]);
    assert!(ok_dry, "dry-run cleanup should exit 0; stderr={dry_err}");
    assert!(
        dry_out.contains("DRY RUN") && dry_out.contains(&plan_id),
        "dry-run must name the flagged plan; got:\n{dry_out}"
    );
    assert!(
        dry_out.contains("would delete"),
        "dry-run must use 'would delete' wording; got:\n{dry_out}"
    );

    // Still present after dry-run.
    let still = scalar(
        &format!("SELECT COUNT(*) AS n FROM plan_metadata WHERE plan_id = '{plan_id}'"),
        "n",
    );
    assert_eq!(
        still.as_deref(),
        Some("1"),
        "dry-run must NOT delete the plan_metadata row"
    );

    // Real wipe.
    let (ok_wipe, wipe_out, wipe_err) = run_cmd(&["db", "cleanup-deleted", "--confirm"]);
    assert!(ok_wipe, "wipe should exit 0; stderr={wipe_err}");
    assert!(
        wipe_out.contains("deleted") && wipe_out.contains(&plan_id),
        "wipe must report the deletion; got:\n{wipe_out}"
    );

    // Now gone from every plan-scoped table.
    let gone_plan = scalar(
        &format!("SELECT COUNT(*) AS n FROM plans WHERE plan_id = '{plan_id}'"),
        "n",
    );
    let gone_meta = scalar(
        &format!("SELECT COUNT(*) AS n FROM plan_metadata WHERE plan_id = '{plan_id}'"),
        "n",
    );
    let gone_anchor = scalar(
        &format!("SELECT COUNT(*) AS n FROM date_anchors WHERE plan_id = '{plan_id}'"),
        "n",
    );

    // `travel plans` no longer shows it (it was already hidden, now truly gone).
    let (_, plans_after, _) = run_cmd(&["plans"]);

    assert_eq!(gone_plan.as_deref(), Some("0"), "plans row must be wiped");
    assert_eq!(gone_meta.as_deref(), Some("0"), "plan_metadata row must be wiped");
    assert_eq!(gone_anchor.as_deref(), Some("0"), "date_anchors row must be wiped");
    assert!(
        !plans_after.contains(&plan_id),
        "wiped plan must not list; got:\n{plans_after}"
    );
}
