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

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_travel"))
}

fn nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn is_credless(stderr: &str) -> bool {
    stderr.contains("turso auth login")
        || stderr.contains("Missing Turso data")
        || stderr.contains("failed to connect to Turso")
        || stderr.contains("TRAVEL_TURSO")
}

/// Run `db exec`; returns Some(stdout) on success, None on a credless skip,
/// panics on a real failure.
fn db_exec(sql: &str) -> Option<String> {
    let out = bin().args(["db", "exec", sql]).output().expect("run travel db exec");
    if out.status.success() {
        return Some(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if is_credless(&stderr) {
        eprintln!("skipping set-activity-poi Turso test: {}", stderr.trim());
        return None;
    }
    panic!("travel db exec failed: {}\nSQL: {sql}", stderr.trim());
}

/// Seed `plans` + `plan_metadata` + a `days` row + activities + one POI.
/// Returns false on a credless skip.
fn seed(plan_id: &str, dest: &str) -> bool {
    let sql = format!(
        "INSERT INTO plans (plan_id, schema_version, version) VALUES ('{plan_id}', '4.2.0', 0); \
         INSERT INTO plan_metadata (plan_id, schema_version, active_destination) \
           VALUES ('{plan_id}', '4.2.0', '{dest}'); \
         INSERT INTO days (plan_id, destination, day_number, date, day_type) \
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
    let sql = format!(
        "DELETE FROM plan_event_data WHERE plan_id = '{plan_id}'; \
         DELETE FROM plan_events WHERE plan_id = '{plan_id}'; \
         DELETE FROM operation_runs WHERE plan_id = '{plan_id}'; \
         DELETE FROM activities WHERE plan_id = '{plan_id}'; \
         DELETE FROM days WHERE plan_id = '{plan_id}'; \
         DELETE FROM destination_pois WHERE slug = '{dest}'; \
         DELETE FROM plan_metadata WHERE plan_id = '{plan_id}'; \
         DELETE FROM plans WHERE plan_id = '{plan_id}';"
    );
    let _ = bin().args(["db", "exec", &sql]).output();
}

/// Run a `travel` subcommand with TRAVEL_PLAN_ID set. Returns (ok, stdout, stderr).
fn run_cmd(plan_id: &str, args: &[&str]) -> (bool, String, String) {
    let out = bin()
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
    let out = db_exec(sql)?;
    let n = out
        .lines()
        .find_map(|l| l.strip_prefix("n: "))
        .map(|s| s.trim().parse::<i64>().unwrap_or(-1))
        .unwrap_or(0);
    Some(n)
}

// ── Links an activity to a poi_id DESPITE a divergent title (the core fix) ───
#[test]
fn set_activity_poi_links_by_id_with_match_disambiguator() {
    let tag = nanos();
    let plan_id = format!("test-actpoi-link-{tag}");
    let dest = format!("actpoilink_{tag}");
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
    ));
    // The OTHER day-2-morning activity (Kinjo) must remain unlinked.
    let other = db_exec(&format!(
        "SELECT poi_id FROM activities \
         WHERE plan_id = '{plan_id}' AND id = '{plan_id}-a2'"
    ));
    let audit = count(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs \
         WHERE plan_id = '{plan_id}' AND command_type = 'set-activity-poi' AND status = 'completed'"
    ));
    teardown(&plan_id, &dest);

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
    teardown(&plan_id, &dest);

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
    teardown(&plan_id, &dest);

    assert!(!ok, "unknown poi_id must exit non-zero; stderr={stderr}");
    assert_eq!(any_linked, Some(0), "no activity may be linked to an unknown poi_id");
    assert_eq!(audit, Some(0), "no completed audit row on an unknown poi_id");
}
