//! Fail-loud guard against BROKEN embedded Google-Maps URLs in activity titles.
//!
//! The dashboard's activity-text linkifier takes an embedded URL token verbatim;
//! the `/maps/dir/?...&...` query form breaks because the linkifier truncates at
//! the first `&`, producing a dead link. The path form (`/maps/search/<place>`)
//! and the `?q=lat,lon` form have no `&`, so they survive.
//!
//! These tests exercise the COMMAND surface (`add-activity`) end-to-end against
//! real Turso: a title carrying the `/maps/dir/?...&...` form must be REJECTED
//! (non-zero exit, NOTHING written); a plain title and a clean `/maps/search/`
//! title must pass (the activity row lands).
//!
//! Pattern mirrors set_mutation_bugs.rs: seed a throwaway plan + a `days` row so
//! the write can succeed, run the binary, SELECT to assert, tear down. Skips
//! cleanly when Turso creds are absent. NEVER touches a real plan.

use std::process::Command;

mod common;
use common::{bin, db_exec, db_exec_teardown, nanos, seed_plan, teardown_plan, Guard};

/// Seed `plans` + `plan_metadata` + one `days` row (day 1, full) so an
/// add-activity write can succeed. Returns false on a credless skip.
fn seed_with_day(plan_id: &str, dest: &str) -> bool {
    if db_exec("SELECT 1").is_none() {
        return false;
    }
    seed_plan(plan_id, dest, 0);
    let sql = format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type) \
           VALUES ('{plan_id}', '{dest}', 1, '2026-06-12', 'full');"
    );
    db_exec(&sql).is_some()
}

fn teardown(plan_id: &str) {
    let _ = db_exec_teardown(&format!(
        "DELETE FROM activity_tags WHERE activity_id IN \
          (SELECT id FROM activities WHERE plan_id = '{plan_id}');"
    ));
    teardown_plan(plan_id, "");
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

// ── REJECT: a /maps/dir/?...&... title is refused, NOTHING written ───────────
#[test]
fn add_activity_rejects_broken_dir_map_url_in_title() {
    let tag = nanos();
    let plan_id = format!("test-maplink-reject-{tag}");
    let _g = Guard::new({
        let plan_id = plan_id.clone();
        move || teardown(&plan_id)
    });
    let dest = format!("maplinkreject_{tag}");
    if !seed_with_day(&plan_id, &dest) {
        return;
    }

    // 'Google Maps：https://www.google.com/maps/dir/?...&...' → broken link form.
    let title =
        "首里城 Google Maps：https://www.google.com/maps/dir/?api=1&destination=Shuri+Castle";
    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &["add-activity", "1", "morning", title, "--dest", &dest],
    );

    let act = count(&format!(
        "SELECT COUNT(*) AS n FROM activities WHERE plan_id = '{plan_id}'"
    ));
    let audit = count(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs \
         WHERE plan_id = '{plan_id}' AND command_type = 'add-activity' AND status = 'completed'"
    ));

    assert!(
        !ok,
        "add-activity must exit non-zero for a broken /maps/dir/?...&... title; stdout={stdout} stderr={stderr}"
    );
    assert!(
        stderr.contains('&') && stderr.to_lowercase().contains("map url"),
        "the error must name the offending '&' map URL; stderr={stderr}"
    );
    assert_eq!(
        act,
        Some(0),
        "NO activities row may be written when the title's map URL is broken"
    );
    assert_eq!(
        audit,
        Some(0),
        "NO completed add-activity audit row may be written when nothing was inserted"
    );
}

// ── PASS: a plain title (no embedded URL) is accepted ────────────────────────
#[test]
fn add_activity_accepts_plain_title() {
    let tag = nanos();
    let plan_id = format!("test-maplink-plain-{tag}");
    let _g = Guard::new({
        let plan_id = plan_id.clone();
        move || teardown(&plan_id)
    });
    let dest = format!("maplinkplain_{tag}");
    if !seed_with_day(&plan_id, &dest) {
        return;
    }

    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &["add-activity", "1", "morning", "Shuri Castle stroll", "--dest", &dest],
    );

    let act = count(&format!(
        "SELECT COUNT(*) AS n FROM activities WHERE plan_id = '{plan_id}'"
    ));

    assert!(
        ok,
        "add-activity must accept a plain title with no embedded URL; stdout={stdout} stderr={stderr}"
    );
    assert_eq!(act, Some(1), "a plain-title activity row must be persisted");
}

// ── PASS: a clean /maps/search/ path-form URL (no '&') is accepted ───────────
#[test]
fn add_activity_accepts_clean_search_map_url() {
    let tag = nanos();
    let plan_id = format!("test-maplink-search-{tag}");
    let _g = Guard::new({
        let plan_id = plan_id.clone();
        move || teardown(&plan_id)
    });
    let dest = format!("maplinksearch_{tag}");
    if !seed_with_day(&plan_id, &dest) {
        return;
    }

    // Path-form /maps/search/<place> has no '&' → must pass.
    let title = "首里城 Google Maps：https://www.google.com/maps/search/Shuri+Castle";
    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &["add-activity", "1", "morning", title, "--dest", &dest],
    );

    let act = count(&format!(
        "SELECT COUNT(*) AS n FROM activities WHERE plan_id = '{plan_id}'"
    ));

    assert!(
        ok,
        "add-activity must accept a clean /maps/search/ path-form URL; stdout={stdout} stderr={stderr}"
    );
    assert_eq!(
        act,
        Some(1),
        "a clean-map-URL-title activity row must be persisted"
    );
}