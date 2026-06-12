//! Fail-loud guard against BROKEN embedded Google-Maps URLs in meal pins.
//!
//! A meal may carry a place link: either the "<label>｜map:<query>" marker (the
//! dashboard renders that as a clean `/maps/search/` link, always '&'-free) or an
//! embedded Maps URL. The `/maps/dir/?...&...` URL form is broken — the dashboard
//! linkifier truncates it at the first `&`, producing a dead link. `set-meals`
//! must REJECT that form at write time (non-zero exit, NOTHING written), exactly
//! like `add-activity`/`set-activity-title` do for activity titles. This closes
//! the last unguarded map-link write path (lint-shift-left audit item #8).
//!
//! Pattern mirrors map_link_title_guard.rs: seed a throwaway plan + a `days` row +
//! a `timesofday` session row (set-meals' require_session needs it), run the
//! binary, SELECT to assert, tear down. Skips cleanly when Turso creds are absent.
//! NEVER touches a real plan.

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

/// Run `db exec`; Some(stdout) on success, None on a credless skip, panic on a
/// real failure.
fn db_exec(sql: &str) -> Option<String> {
    let out = bin().args(["db", "exec", sql]).output().expect("run travel db exec");
    if out.status.success() {
        return Some(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if is_credless(&stderr) {
        eprintln!("skipping meal-link-guard Turso test: {}", stderr.trim());
        return None;
    }
    panic!("travel db exec failed: {}\nSQL: {sql}", stderr.trim());
}

/// Seed `plans` + `plan_metadata` + one `days` row + one `timesofday` session row
/// (day 1, noon) so a set-meals write can succeed. Returns false on a credless skip.
fn seed_with_session(plan_id: &str, dest: &str) -> bool {
    let sql = format!(
        "INSERT INTO plans (plan_id, schema_version, version) VALUES ('{plan_id}', '4.2.0', 0); \
         INSERT INTO plan_metadata (plan_id, schema_version, active_destination) \
           VALUES ('{plan_id}', '4.2.0', '{dest}'); \
         INSERT INTO days (plan_id, destination, day_number, date, day_type) \
           VALUES ('{plan_id}', '{dest}', 1, '2026-06-12', 'full'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type, focus) \
           VALUES ('{plan_id}', '{dest}', 1, 'noon', 'lunch');"
    );
    db_exec(&sql).is_some()
}

fn teardown(plan_id: &str) {
    let sql = format!(
        "DELETE FROM plan_event_data WHERE plan_id = '{plan_id}'; \
         DELETE FROM plan_events WHERE plan_id = '{plan_id}'; \
         DELETE FROM operation_runs WHERE plan_id = '{plan_id}'; \
         DELETE FROM session_meals WHERE plan_id = '{plan_id}'; \
         DELETE FROM timesofday WHERE plan_id = '{plan_id}'; \
         DELETE FROM days WHERE plan_id = '{plan_id}'; \
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

// ── REJECT: a /maps/dir/?...&... meal pin is refused, NOTHING written ─────────
#[test]
fn set_meals_rejects_broken_dir_map_url() {
    let tag = nanos();
    let plan_id = format!("test-meal:link-reject-{tag}").replace(':', "-");
    let dest = format!("meallinkreject_{tag}");
    if !seed_with_session(&plan_id, &dest) {
        return;
    }

    // 'Google Maps：https://www.google.com/maps/dir/?...&...' → broken link form.
    let meal =
        "午餐：首里殿内 Google Maps：https://www.google.com/maps/dir/?api=1&destination=Shuri";
    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &["set-meals", "1", "noon", "--meal", meal, "--dest", &dest],
    );

    let meals = count(&format!(
        "SELECT COUNT(*) AS n FROM session_meals WHERE plan_id = '{plan_id}'"
    ));
    let audit = count(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs \
         WHERE plan_id = '{plan_id}' AND command_type = 'set-meals' AND status = 'completed'"
    ));
    teardown(&plan_id);

    assert!(
        !ok,
        "set-meals must exit non-zero for a broken /maps/dir/?...&... meal pin; stdout={stdout} stderr={stderr}"
    );
    assert!(
        stderr.contains('&') && stderr.to_lowercase().contains("map url"),
        "the error must name the offending '&' map URL; stderr={stderr}"
    );
    assert_eq!(
        meals,
        Some(0),
        "NO session_meals row may be written when the meal's map URL is broken"
    );
    assert_eq!(
        audit,
        Some(0),
        "NO completed set-meals audit row may be written when nothing was inserted"
    );
}

// ── PASS: a plain meal (no embedded URL) is accepted ─────────────────────────
#[test]
fn set_meals_accepts_plain_meal() {
    let tag = nanos();
    let plan_id = format!("test-meallink-plain-{tag}");
    let dest = format!("meallinkplain_{tag}");
    if !seed_with_session(&plan_id, &dest) {
        return;
    }

    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &["set-meals", "1", "noon", "--meal", "午餐：機上餐", "--dest", &dest],
    );

    let meals = count(&format!(
        "SELECT COUNT(*) AS n FROM session_meals WHERE plan_id = '{plan_id}'"
    ));
    teardown(&plan_id);

    assert!(
        ok,
        "set-meals must accept a plain meal with no embedded URL; stdout={stdout} stderr={stderr}"
    );
    assert_eq!(meals, Some(1), "a plain meal row must be persisted");
}

// ── PASS: the ｜map:<query> marker form (renders as clean /maps/search/) ──────
#[test]
fn set_meals_accepts_map_marker_pin() {
    let tag = nanos();
    let plan_id = format!("test-meallink-marker-{tag}");
    let dest = format!("meallinkmarker_{tag}");
    if !seed_with_session(&plan_id, &dest) {
        return;
    }

    // The "<label>｜map:<query>" convention — no embedded URL, no '&' → must pass.
    let meal = "午餐：首里殿内｜map:首里殿内";
    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &["set-meals", "1", "noon", "--meal", meal, "--dest", &dest],
    );

    let meals = count(&format!(
        "SELECT COUNT(*) AS n FROM session_meals WHERE plan_id = '{plan_id}'"
    ));
    teardown(&plan_id);

    assert!(
        ok,
        "set-meals must accept the ｜map:<query> marker pin form; stdout={stdout} stderr={stderr}"
    );
    assert_eq!(meals, Some(1), "a map-marker meal row must be persisted");
}

// ── PASS: a clean /maps/search/ path-form URL (no '&') is accepted ───────────
#[test]
fn set_meals_accepts_clean_search_map_url() {
    let tag = nanos();
    let plan_id = format!("test-meallink-search-{tag}");
    let dest = format!("meallinksearch_{tag}");
    if !seed_with_session(&plan_id, &dest) {
        return;
    }

    // Path-form /maps/search/<place> has no '&' → must pass.
    let meal = "晚餐：A Google Maps：https://www.google.com/maps/search/Naha+Dinner";
    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &["set-meals", "1", "noon", "--meal", meal, "--dest", &dest],
    );

    let meals = count(&format!(
        "SELECT COUNT(*) AS n FROM session_meals WHERE plan_id = '{plan_id}'"
    ));
    teardown(&plan_id);

    assert!(
        ok,
        "set-meals must accept a clean /maps/search/ path-form URL; stdout={stdout} stderr={stderr}"
    );
    assert_eq!(meals, Some(1), "a clean-map-URL meal row must be persisted");
}
