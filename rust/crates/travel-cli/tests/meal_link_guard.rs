//! Fail-loud guard against BROKEN embedded Google-Maps URLs in meal text.
//!
//! A meal may carry a place link via the "<label>｜map:<query>" marker (always
//! '&'-free) or an embedded Maps URL. The `/maps/dir/?...&...` URL form is broken:
//! the dashboard linkifier truncates at the first `&`. `set-meals` must REJECT it
//! at write time (non-zero exit, NOTHING written), like `add-activity` does for
//! titles (see map_link_title_guard.rs). Plain meals, the ｜map: marker, and a
//! clean `/maps/search/` URL must pass. Skips cleanly when Turso creds are absent.

use std::process::Command;

mod common;
use common::{bin, db_exec, nanos, seed_plan, teardown_plan, Guard};

/// Seed a plan + day 1 + a `noon` timesofday row (set-meals' require_session).
/// Returns false on a credless skip.
fn seed_with_session(plan_id: &str, dest: &str) -> bool {
    if db_exec("SELECT 1").is_none() {
        return false;
    }
    seed_plan(plan_id, dest, 0);
    db_exec(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type) \
           VALUES ('{plan_id}', '{dest}', 1, '2026-06-12', 'full'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type) \
           VALUES ('{plan_id}', '{dest}', 1, 'noon');"
    ))
    .is_some()
}

fn run_meal(plan_id: &str, dest: &str, meal: &str) -> (bool, String) {
    let out = Command::new(bin())
        .args(["set-meals", "1", "noon", "--meal", meal, "--dest", dest])
        .env("TRAVEL_PLAN_ID", plan_id)
        .output()
        .expect("run travel set-meals");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn meal_count(plan_id: &str) -> i64 {
    db_exec(&format!(
        "SELECT COUNT(*) AS n FROM session_meals WHERE plan_id = '{plan_id}'"
    ))
    .and_then(|r| r.scalar())
    .and_then(|s| s.parse().ok())
    .unwrap_or(-1)
}

/// Seed, run one set-meals, return (ok, stderr, rows written); None = credless skip.
fn case(label: &str, meal: &str) -> Option<(bool, String, i64)> {
    let tag = nanos();
    let plan_id = format!("test-meallink-{label}-{tag}");
    let dest = format!("meallink{label}_{tag}");
    let _g = Guard::new({
        let (p, d) = (plan_id.clone(), dest.clone());
        move || teardown_plan(&p, &d)
    });
    if !seed_with_session(&plan_id, &dest) {
        return None;
    }
    let (ok, stderr) = run_meal(&plan_id, &dest, meal);
    Some((ok, stderr, meal_count(&plan_id)))
}

#[test]
fn set_meals_rejects_broken_dir_map_url() {
    let Some((ok, stderr, n)) = case(
        "reject",
        "午餐：首里殿内 Google Maps：https://www.google.com/maps/dir/?api=1&destination=Shuri",
    ) else {
        return;
    };
    assert!(!ok, "broken /maps/dir/?...&... meal must be rejected; stderr={stderr}");
    assert!(
        stderr.contains('&') && stderr.to_lowercase().contains("map url"),
        "error must name the offending '&' map URL; stderr={stderr}"
    );
    assert_eq!(n, 0, "no session_meals row may be written");
}

#[test]
fn set_meals_accepts_plain_meal() {
    let Some((ok, stderr, n)) = case("plain", "午餐：機上餐") else { return };
    assert!(ok, "plain meal must pass; stderr={stderr}");
    assert_eq!(n, 1);
}

#[test]
fn set_meals_accepts_map_marker_pin() {
    let Some((ok, stderr, n)) = case("marker", "午餐：首里殿内｜map:首里殿内") else {
        return;
    };
    assert!(ok, "｜map: marker must pass; stderr={stderr}");
    assert_eq!(n, 1);
}

#[test]
fn set_meals_accepts_clean_search_map_url() {
    let Some((ok, stderr, n)) = case(
        "search",
        "晚餐：A Google Maps：https://www.google.com/maps/search/Naha+Dinner",
    ) else {
        return;
    };
    assert!(ok, "clean /maps/search/ URL must pass; stderr={stderr}");
    assert_eq!(n, 1);
}
