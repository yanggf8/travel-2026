//! Integration tests for `travel set-activity-poi` — the durable activity→POI
//! link that replaces fragile title-based price matching on the dashboard.
//!
//! Invariants exercised:
//!   1. Linking a seeded activity to a real poi_id persists `activities.poi_id`
//!      (a SELECT confirms), even when the activity title differs from the POI
//!      title (the whole point of the FK).
//!   2. `--match <substring>` disambiguates when a (day, session) holds >1
//!      activity; an ambiguous call without `--match` fails loud.
//!   3. An unknown poi_id fails loud (non-zero) and writes NO link / no
//!      completed audit row.
//!
//! Pattern mirrors set_mutation_bugs.rs: seed a throwaway plan, run the binary,
//! SELECT to assert, tear down. Skips cleanly when Turso creds are absent.

mod common;
use common::{bin, db_exec, db_exec_teardown, nanos, seed_plan, teardown_plan, Guard};

use std::process::Command;

/// Seed a `days` row + activities + one POI.
/// Returns false on a credless skip.
fn seed(plan_id: &str, dest: &str) -> bool {
    if db_exec("SELECT 1").is_none() {
        return false;
    }
    seed_plan(plan_id, dest, 0);
    let sql = format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type) \
           VALUES ('{plan_id}', '{dest}', 2, '2026-06-13', 'full'); \
         INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title) \
           VALUES ('{plan_id}-a1', '{plan_id}', '{dest}', 2, 'morning', 0, \
                   'Shurijo Castle Park (首里城公園) — reconstruction grounds'); \
         INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title) \
           VALUES ('{plan_id}-a2', '{plan_id}', '{dest}', 2, 'morning', 1, \
                   'Kinjo-cho Stone Path (金城町石畳道)'); \
         INSERT INTO destination_pois (slug, poi_id, title, cost_estimate) \
           VALUES ('{dest}', 'shuri_castle', 'Shuri Castle (首里城)', 400);"
    );
    db_exec(&sql).is_some()
}

fn teardown(plan_id: &str, dest: &str) {
    teardown_plan(plan_id, dest);
    let _ = db_exec_teardown(&format!("DELETE FROM destination_pois WHERE slug = '{dest}';"));
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

/// COUNT(*) helper. Returns the integer count, or None on a credless skip.
fn count(sql: &str) -> Option<i64> {
    let n = db_exec(sql)?
        .scalar()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    Some(n)
}

// ── Links an activity to a poi_id DESPITE a divergent title (the core fix) ───
#[test]
fn set_activity_poi_links_by_id_with_match_disambiguator() {
    let tag = nanos();
    let plan_id = format!("test-actpoi-link-{tag}");
    let dest = format!("actpoilink_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown(&plan_id, &dest)
    });
    if !seed(&plan_id, &dest) {
        return;
    }

    // Day 2 morning has TWO activities → disambiguate with --match "Shurijo".
    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &["set-activity-poi", "2", "morning", "shuri_castle", "--match", "Shurijo", "--dest", &dest],
    );

    let linked = db_exec(&format!(
        "SELECT poi_id FROM activities \
         WHERE plan_id = '{plan_id}' AND id = '{plan_id}-a1'"
    ))
    .map(|rows| rows.raw().to_string());
    // The OTHER day-2-morning activity (Kinjo) must remain unlinked.
    let other = db_exec(&format!(
        "SELECT poi_id FROM activities \
         WHERE plan_id = '{plan_id}' AND id = '{plan_id}-a2'"
    ))
    .map(|rows| rows.raw().to_string());
    let audit = count(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs \
         WHERE plan_id = '{plan_id}' AND command_type = 'set-activity-poi' AND status = 'completed'"
    ));

    assert!(ok, "set-activity-poi should succeed; stdout={stdout} stderr={stderr}");
    assert!(
        linked.unwrap_or_default().contains("poi_id: shuri_castle"),
        "the Shurijo activity must be linked to poi_id=shuri_castle"
    );
    assert!(
        !other.unwrap_or_default().contains("poi_id: shuri_castle"),
        "the sibling Kinjo activity must remain unlinked"
    );
    assert_eq!(audit, Some(1), "exactly one completed audit row must be written");
}

// ── Ambiguous (day, session) with no --match → fail loud, no write ───────────
#[test]
fn set_activity_poi_fails_loud_when_ambiguous() {
    let tag = nanos();
    let plan_id = format!("test-actpoi-ambig-{tag}");
    let dest = format!("actpoiambig_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown(&plan_id, &dest)
    });
    if !seed(&plan_id, &dest) {
        return;
    }

    // Two activities in day 2 morning, no --match → must fail loud.
    let (ok, _stdout, stderr) = run_cmd(
        &plan_id,
        &["set-activity-poi", "2", "morning", "shuri_castle", "--dest", &dest],
    );

    let any_linked = count(&format!(
        "SELECT COUNT(*) AS n FROM activities \
         WHERE plan_id = '{plan_id}' AND poi_id IS NOT NULL"
    ));
    let audit = count(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs \
         WHERE plan_id = '{plan_id}' AND command_type = 'set-activity-poi' AND status = 'completed'"
    ));

    assert!(!ok, "ambiguous match must exit non-zero; stderr={stderr}");
    assert_eq!(any_linked, Some(0), "no activity may be linked on an ambiguous call");
    assert_eq!(audit, Some(0), "no completed audit row on an ambiguous call");
}

// ── Unknown poi_id → fail loud, no write ─────────────────────────────────────
#[test]
fn set_activity_poi_fails_loud_on_unknown_poi() {
    let tag = nanos();
    let plan_id = format!("test-actpoi-bad-{tag}");
    let dest = format!("actpoibad_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown(&plan_id, &dest)
    });
    if !seed(&plan_id, &dest) {
        return;
    }

    let (ok, _stdout, stderr) = run_cmd(
        &plan_id,
        &["set-activity-poi", "2", "morning", "no_such_poi", "--match", "Shurijo", "--dest", &dest],
    );

    let any_linked = count(&format!(
        "SELECT COUNT(*) AS n FROM activities \
         WHERE plan_id = '{plan_id}' AND poi_id IS NOT NULL"
    ));
    let audit = count(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs \
         WHERE plan_id = '{plan_id}' AND command_type = 'set-activity-poi' AND status = 'completed'"
    ));

    assert!(!ok, "unknown poi_id must exit non-zero; stderr={stderr}");
    assert_eq!(any_linked, Some(0), "no activity may be linked to an unknown poi_id");
    assert_eq!(audit, Some(0), "no completed audit row on an unknown poi_id");
}