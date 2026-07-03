//! Real-Turso integration tests for two sort_order-manipulating mutations added
//! after the okinawa editing session:
//!   - `move-activity` re-keys an activity to another session, preserving its id
//!     (and poi_id), instead of delete+re-add.
//!   - `add-activity --after <id|title>` inserts at a position instead of always
//!     appending.
//!
//! Harness mirrors tests/swap_days.rs: seed a nanos-tagged throwaway plan, run
//! the binary, SELECT to assert, tear down. Skips cleanly when creds are absent.

use std::process::Command;

mod common;
use common::{bin, db_exec, db_exec_teardown, nanos, seed_plan, teardown_plan, Guard};
fn run_cmd(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin()).args(args).env_remove("TRAVEL_PLAN_ID").output().expect("run travel");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}
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

/// Seed plan + metadata + two days, each with morning+afternoon timesofday rows.
fn seed(plan_id: &str, dest: &str) -> bool {
    if db_exec("SELECT 1 AS n").is_none() {
        return false;
    }
    seed_plan(plan_id, dest, 0);
    let sql = format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type, status) VALUES ('{plan_id}', '{dest}', 1, '2026-09-01', 'arrival', 'draft'); \
         INSERT INTO days (plan_id, destination, day_number, date, day_type, status) VALUES ('{plan_id}', '{dest}', 2, '2026-09-02', 'full', 'draft'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type) VALUES ('{plan_id}', '{dest}', 1, 'morning'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type) VALUES ('{plan_id}', '{dest}', 1, 'afternoon'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type) VALUES ('{plan_id}', '{dest}', 2, 'morning');"
    );
    db_exec(&sql).is_some()
}
fn teardown(plan_id: &str, dest: &str) {
    let _ = db_exec_teardown(&format!(
        "DELETE FROM activity_tags WHERE activity_id IN (SELECT id FROM activities WHERE plan_id='{plan_id}');"));
    teardown_plan(plan_id, dest);
}

// ── move-activity preserves the id while re-keying day/session ─────────────────
#[test]
fn move_activity_preserves_id_across_sessions() {
    let tag = nanos();
    let plan_id = format!("test-move-{tag}");
    let dest = format!("move_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown(&plan_id, &dest)
    });
    if !seed(&plan_id, &dest) {
        return;
    }

    // Add an activity to D1 morning.
    let (ok, out, err) =
        run_cmd(&["add-activity", "1", "morning", "Aquarium visit", "--plan-id", &plan_id, "--dest", &dest]);
    assert!(ok, "add failed: {out}{err}");
    let id_before = scalar(
        &format!("SELECT id AS i FROM activities WHERE plan_id='{plan_id}' AND title='Aquarium visit'"),
        "i",
    )
    .unwrap_or_default();
    assert!(!id_before.is_empty(), "seeded activity should have an id");

    // Move it to D1 afternoon.
    let (mok, mout, merr) = run_cmd(&[
        "move-activity", "1", "morning", "afternoon", "Aquarium visit", "--plan-id", &plan_id, "--dest", &dest,
    ]);
    assert!(mok, "move failed: {mout}{merr}");

    // Now lives in afternoon, SAME id, gone from morning.
    let id_after = scalar(
        &format!("SELECT id AS i FROM activities WHERE plan_id='{plan_id}' AND day_number=1 AND session_type='afternoon' AND title='Aquarium visit'"),
        "i",
    );
    let still_morning = scalar(
        &format!("SELECT COUNT(*) AS n FROM activities WHERE plan_id='{plan_id}' AND day_number=1 AND session_type='morning'"),
        "n",
    );

    assert_eq!(id_after.as_deref(), Some(id_before.as_str()), "id must be preserved by move");
    assert_eq!(still_morning.as_deref(), Some("0"), "activity must leave the source session");
}

// ── add-activity --after inserts at the right slot ────────────────────────────
#[test]
fn add_activity_after_inserts_in_order() {
    let tag = nanos();
    let plan_id = format!("test-after-{tag}");
    let dest = format!("after_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown(&plan_id, &dest)
    });
    if !seed(&plan_id, &dest) {
        return;
    }

    // Build morning: A (0), B (1).
    for title in ["Alpha", "Bravo"] {
        let (ok, o, e) = run_cmd(&["add-activity", "1", "morning", title, "--plan-id", &plan_id, "--dest", &dest]);
        assert!(ok, "add {title} failed: {o}{e}");
    }
    // Insert Charlie AFTER Alpha → order should be Alpha, Charlie, Bravo.
    let (ok, o, e) = run_cmd(&[
        "add-activity", "1", "morning", "Charlie", "--after", "Alpha", "--plan-id", &plan_id, "--dest", &dest,
    ]);
    assert!(ok, "add --after failed: {o}{e}");

    // Read titles ordered by sort_order.
    let rows = db_exec(&format!(
        "SELECT title FROM activities WHERE plan_id='{plan_id}' AND day_number=1 AND session_type='morning' ORDER BY sort_order"
    ))
    .expect("select ordered")
    .raw()
    .to_string();
    let order: Vec<String> = rows
        .lines()
        .filter_map(|l| l.strip_prefix("title: ").map(|s| s.trim().to_string()))
        .collect();

    assert_eq!(
        order,
        vec!["Alpha".to_string(), "Charlie".to_string(), "Bravo".to_string()],
        "Charlie must sit between Alpha and Bravo"
    );
}
