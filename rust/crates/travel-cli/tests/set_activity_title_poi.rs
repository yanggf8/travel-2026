//! Regression tests for the title↔poi_id inconsistency bug in
//! `set-activity-title` (see docs/handoff-set-activity-title-poi-bug.md).
//!
//! Invariant: re-theming an activity by editing its title MUST NOT leave a
//! stale `poi_id` pointing at the OLD place. The fix (clear-and-notify):
//!   - When the title actually CHANGES and the row currently HAS a poi_id,
//!     the UPDATE also sets `poi_id = NULL` and prints an agent-first note
//!     naming the cleared id + a copy-pasteable re-link command.
//!   - An idempotent re-run (same title) leaves poi_id untouched, no note.
//!   - A row with no poi_id behaves exactly as before (just retitle, no note).
//!
//! Pattern mirrors set_mutation_bugs.rs: seed a throwaway plan + a single
//! activity row, run the binary, SELECT to assert, tear down. Skips cleanly
//! when Turso creds are absent. NEVER touches a real plan.

mod common;
use common::{bin, db_exec, nanos, seed_plan, teardown_plan, Guard};

use std::process::Command;

/// Seed `plans` + `plan_metadata` + a day + a session + ONE activity row whose
/// id is `act_id` (unique per test so concurrent tests don't collide on the
/// `activities.id` PK). When `poi_id` is Some, the activity carries that POI
/// link. Returns false on a credless skip (caller should early-return).
fn seed_activity(
    plan_id: &str,
    dest: &str,
    act_id: &str,
    title: &str,
    poi_id: Option<&str>,
) -> bool {
    if db_exec("SELECT 1").is_none() {
        return false;
    }
    seed_plan(plan_id, dest, 0);
    let poi_clause = match poi_id {
        Some(p) => format!("'{p}'"),
        None => "NULL".to_string(),
    };
    let sql = format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type) \
           VALUES ('{plan_id}', '{dest}', 1, '2026-06-12', 'full'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type) \
           VALUES ('{plan_id}', '{dest}', 1, 'morning'); \
         INSERT INTO activities \
           (id, plan_id, destination, day_number, session_type, sort_order, title, poi_id, priority) \
           VALUES ('{act_id}', '{plan_id}', '{dest}', 1, 'morning', 0, '{title}', {poi_clause}, 'want');"
    );
    db_exec(&sql).is_some()
}

fn teardown(plan_id: &str, dest: &str) {
    teardown_plan(plan_id, dest);
}

/// Run a `travel` subcommand with TRAVEL_PLAN_ID set. Returns (ok, stdout, stderr).
fn run_cmd(plan_id: &str, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(args)
        .env("TRAVEL_PLAN_ID", plan_id)
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Read the current poi_id of the seeded activity. Returns:
///   Some("<id>")  when set, Some("") when NULL/empty, None on a credless skip.
fn read_poi_id(plan_id: &str, act_id: &str) -> Option<String> {
    let val = db_exec(&format!(
        "SELECT poi_id FROM activities WHERE plan_id = '{plan_id}' AND id = '{act_id}'"
    ))?
    .scalar()
    .unwrap_or_default();
    Some(val)
}

/// Read the current title of the seeded activity. None on a credless skip.
fn read_title(plan_id: &str, act_id: &str) -> Option<String> {
    let val = db_exec(&format!(
        "SELECT title FROM activities WHERE plan_id = '{plan_id}' AND id = '{act_id}'"
    ))?
    .scalar()
    .unwrap_or_default();
    Some(val)
}

// ── Case 1: title CHANGES + poi_id set → poi_id cleared + agent-first note ────
#[test]
fn title_change_clears_stale_poi_id_and_notifies() {
    let tag = nanos();
    let plan_id = format!("test-acttitle-clear-{tag}");
    let dest = format!("acttitleclear_{tag}");
    let act_id = format!("act-clear-{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown(&plan_id, &dest)
    });
    if !seed_activity(&plan_id, &dest, &act_id, "Shikinaen Garden", Some("shikinaen")) {
        return;
    }

    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &["set-activity-title", "1", "morning", &act_id, "DMM aquarium", "--dest", &dest],
    );

    let new_title = read_title(&plan_id, &act_id);
    let new_poi = read_poi_id(&plan_id, &act_id);

    assert!(
        ok,
        "set-activity-title should succeed on a title change; stdout={stdout} stderr={stderr}"
    );
    assert_eq!(
        new_title.as_deref(),
        Some("DMM aquarium"),
        "the title must be updated to the new value"
    );
    assert_eq!(
        new_poi.as_deref(),
        Some(""),
        "the stale poi_id must be cleared to NULL on a title change"
    );
    assert!(
        stdout.contains("cleared poi_id") && stdout.contains("shikinaen"),
        "stdout must carry the agent-first note naming the cleared poi_id; stdout={stdout}"
    );
    assert!(
        stdout.contains("set-activity-poi"),
        "the note must include the copy-pasteable re-link command; stdout={stdout}"
    );
}

// ── Case 2: SAME title (idempotent) → poi_id untouched, no note ───────────────
#[test]
fn idempotent_same_title_keeps_poi_id() {
    let tag = nanos();
    let plan_id = format!("test-acttitle-same-{tag}");
    let dest = format!("acttitlesame_{tag}");
    let act_id = format!("act-same-{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown(&plan_id, &dest)
    });
    if !seed_activity(&plan_id, &dest, &act_id, "Shuri Castle", Some("shuri_castle")) {
        return;
    }

    let (_ok, stdout, _stderr) = run_cmd(
        &plan_id,
        &["set-activity-title", "1", "morning", &act_id, "Shuri Castle", "--dest", &dest],
    );

    let new_poi = read_poi_id(&plan_id, &act_id);

    // Whether the command treats a no-op title as success or rejects it, the
    // invariant is the same: a stale-clear must NOT fire when nothing changed.
    assert_eq!(
        new_poi.as_deref(),
        Some("shuri_castle"),
        "an idempotent same-title run must leave poi_id unchanged"
    );
    assert!(
        !stdout.contains("cleared poi_id"),
        "no clear-poi note may print when the title did not change; stdout={stdout}"
    );
}

// ── Case 3: no poi_id to begin with → retitle only, no note ───────────────────
#[test]
fn title_change_without_poi_id_just_retitles() {
    let tag = nanos();
    let plan_id = format!("test-acttitle-nopoi-{tag}");
    let dest = format!("acttitlenopoi_{tag}");
    let act_id = format!("act-nopoi-{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown(&plan_id, &dest)
    });
    if !seed_activity(&plan_id, &dest, &act_id, "Free time", None) {
        return;
    }

    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &["set-activity-title", "1", "morning", &act_id, "Kokusai-dori stroll", "--dest", &dest],
    );

    let new_title = read_title(&plan_id, &act_id);
    let new_poi = read_poi_id(&plan_id, &act_id);

    assert!(
        ok,
        "set-activity-title should succeed when there is no poi_id; stdout={stdout} stderr={stderr}"
    );
    assert_eq!(
        new_title.as_deref(),
        Some("Kokusai-dori stroll"),
        "the title must still be updated"
    );
    assert_eq!(
        new_poi.as_deref(),
        Some(""),
        "poi_id stays NULL (it was never set)"
    );
    assert!(
        !stdout.contains("cleared poi_id"),
        "no clear-poi note may print when there was no poi_id to clear; stdout={stdout}"
    );
}