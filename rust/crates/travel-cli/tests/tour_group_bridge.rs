//! Port of tests/integration/tour-group-bridge.regression.test.ts.
//!
//! The TS test calls the service fn `bridgeAuditSet()` directly. In Rust the
//! bridge runs inside `shaping-adopt --create-plan` (matching the TS
//! adoptCandidateToNewPlan flow), so this test drives that real command and
//! asserts the audit set landed in plan_offers + plan_offer_group_meta.
//!
//! Audit-set SELECTION logic (raw cheapest + quality-floor + top-3 per
//! source/date) is additionally unit-tested in src/tour_group_bridge.rs.
//!
//! dest_region is derived from destination_config.ref_id, so the seeded offers
//! use ref_id 'osaka' (osaka_2026), and the candidate's dest_code is a
//! configured airport (KIX) so adopt's airport validation passes.

use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

mod common;
use common::Guard;

static BRIDGE_LOCK: Mutex<()> = Mutex::new(());

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_travel")
}

fn nanos() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
}

fn db_exec(sql: &str) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(["db", "exec", sql])
        .output()
        .expect("run db exec");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn is_skip(stderr: &str) -> bool {
    stderr.contains("turso auth login")
        || stderr.contains("Missing Turso")
        || stderr.contains("failed to connect to Turso")
}

fn scalar(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()))
}

/// Lines like "id: a1" → collect the values of a single-column SELECT.
fn column(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()))
        .filter(|v| !v.is_empty())
        .collect()
}

fn seed_offer(run: &str, id: &str, source: &str, price: i64, stars: i64) {
    db_exec(&format!(
        "INSERT OR REPLACE INTO shaping_tour_group_offers \
         (run_id, offer_id, source_id, dest_region, depart_date, return_date, nights, \
          price_per_person_twd, title, url, scraped_at, hotel_star_rating, \
          meals_included_count, departure_status) \
         VALUES ('{run}', '{id}', '{source}', 'osaka', '2026-06-20', '2026-06-25', 5, \
          {price}, 'test {id}', 'http://test/{id}', '2026-05-25T00:00:00Z', {stars}, 5, 'guaranteed')"
    ));
}

fn teardown(run: &str, plan: &str, candidate: &str) {
    for sql in [
        format!("DELETE FROM shaping_tour_group_offers WHERE run_id = '{run}'"),
        format!("DELETE FROM shaping_candidates WHERE candidate_id = '{candidate}'"),
        format!("DELETE FROM shaping_research_runs WHERE run_id = '{run}'"),
        format!("DELETE FROM plan_offer_group_meta WHERE plan_id = '{plan}'"),
        format!("DELETE FROM plan_offers WHERE plan_id = '{plan}'"),
        format!("DELETE FROM plans WHERE plan_id = '{plan}'"),
        format!("DELETE FROM plan_metadata WHERE plan_id = '{plan}'"),
        format!("DELETE FROM plan_destinations WHERE plan_id = '{plan}'"),
        format!("DELETE FROM destination_details WHERE plan_id = '{plan}'"),
        format!("DELETE FROM destination_cities WHERE plan_id = '{plan}'"),
        format!("DELETE FROM date_anchors WHERE plan_id = '{plan}'"),
        format!("DELETE FROM process_statuses WHERE plan_id = '{plan}'"),
        format!("DELETE FROM event_log_state WHERE plan_id = '{plan}'"),
        format!("DELETE FROM event_log_destinations WHERE plan_id = '{plan}'"),
        format!("DELETE FROM plan_events WHERE plan_id = '{plan}'"),
        format!("DELETE FROM plan_event_data WHERE plan_id = '{plan}'"),
        format!("DELETE FROM operation_runs WHERE plan_id = '{plan}'"),
    ] {
        let _ = db_exec(&sql);
    }
}

/// Seed a run + candidate, then `shaping-adopt --create-plan` → bridge runs.
/// Returns adopt's (success, stdout, stderr).
fn seed_and_adopt(run: &str, candidate: &str, plan: &str) -> (bool, String, String) {
    db_exec(&format!(
        "INSERT INTO shaping_research_runs \
         (run_id, origin_code, pax, window_start, window_end, currency, exchange_rate_usd_twd, \
          status, created_at, updated_at) \
         VALUES ('{run}', 'TPE', 2, '2026-06-18', '2026-06-25', 'TWD', 32.0, 'ranked', \
          '2026-05-22T00:00:00Z', '2026-05-22T00:00:00Z')"
    ));
    db_exec(&format!(
        "INSERT INTO shaping_candidates \
         (candidate_id, run_id, dest_code, depart_date, return_date, nights, \
          flight_total_twd, leave_days, rank, verdict) \
         VALUES ('{candidate}', '{run}', 'KIX', '2026-06-20', '2026-06-25', 5, 12000, 3, 1, 'good')"
    ));
    let out = Command::new(bin())
        .args([
            "shaping-adopt",
            candidate,
            plan,
            "--create-plan",
            "--dest",
            "osaka_2026",
        ])
        .output()
        .expect("run shaping-adopt --create-plan");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[tokio::test]
async fn bridge_audit_set_into_adopted_plan() {
    let _guard = BRIDGE_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let (ok, _o, err) = db_exec("SELECT 1");
    if !ok && is_skip(&err) {
        eprintln!("skipping tour-group-bridge test (no Turso creds): {}", err.trim());
        return;
    }

    let tag = nanos();
    let run = format!("test-run-bridge-{tag}");
    let candidate = format!("cand-bridge-{tag}");
    let plan = format!("test-plan-bridge-{tag}");

    teardown(&run, &plan, &candidate);
    // Guard runs teardown on return AND on panic (so a failing assert can't leak rows).
    let _g = Guard::new({
        let (run, plan, candidate) = (run.clone(), plan.clone(), candidate.clone());
        move || teardown(&run, &plan, &candidate)
    });

    // Audit-set fixture (same shape as the TS test): a1 raw cheapest + top-3,
    // a2 quality-floor (4-star) + top-3, a3 top-3, a4 5-star NOT in any set,
    // b1 top-3 for the lifetour source.
    seed_offer(&run, "a1", "besttour", 28000, 3);
    seed_offer(&run, "a2", "besttour", 35000, 4);
    seed_offer(&run, "a3", "besttour", 30000, 3);
    seed_offer(&run, "a4", "besttour", 42000, 5);
    seed_offer(&run, "b1", "lifetour", 31000, 3);

    let (ok, stdout, stderr) = seed_and_adopt(&run, &candidate, &plan);
    if !ok && is_skip(&stderr) {
        eprintln!("skipping (no creds mid-test): {}", stderr.trim());
        return;
    }
    assert!(ok, "shaping-adopt --create-plan should succeed; stderr={stderr} stdout={stdout}");

    // plan_offers: the audit set (a1,a2,a3,b1), NOT a4. All package_subtype=group_tour.
    let (_, ids_out, _) = db_exec(&format!(
        "SELECT id FROM plan_offers WHERE plan_id = '{plan}' ORDER BY id"
    ));
    let ids = column(&ids_out);
    for want in ["a1", "a2", "a3", "b1"] {
        assert!(ids.iter().any(|i| i == want), "plan_offers should contain {want}; got {ids:?}");
    }
    assert!(!ids.iter().any(|i| i == "a4"), "plan_offers must NOT contain a4 (4th cheapest); got {ids:?}");

    let (_, subtypes, _) = db_exec(&format!(
        "SELECT DISTINCT package_subtype FROM plan_offers WHERE plan_id = '{plan}'"
    ));
    assert_eq!(scalar(&subtypes).as_deref(), Some("group_tour"), "all bridged offers are group_tour");

    // plan_offer_group_meta: source_offer_run_id = run for each bridged offer.
    let (_, meta_run, _) = db_exec(&format!(
        "SELECT DISTINCT source_offer_run_id FROM plan_offer_group_meta WHERE plan_id = '{plan}'"
    ));
    assert_eq!(scalar(&meta_run).as_deref(), Some(run.as_str()), "group_meta carries source run_id");
    let (_, meta_n, _) = db_exec(&format!(
        "SELECT COUNT(*) FROM plan_offer_group_meta WHERE plan_id = '{plan}'"
    ));
    assert_eq!(scalar(&meta_n).as_deref(), Some("4"), "4 audit offers → 4 group_meta rows");
}
