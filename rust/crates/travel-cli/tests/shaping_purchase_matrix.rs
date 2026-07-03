//! Real-Turso integration test for `travel shaping-purchase-matrix` (the purchase decision matrix).
//! Seeds a throwaway `zz_...` shaping run (rules + one flight candidate + package offers), runs the
//! command, and asserts the GATE/NUDGE/sort/COST_SCOPE behavior from the impl plan. Panic-safe Guard
//! teardown; skips cleanly if Turso creds are absent. Real plans/runs are never touched.

use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{bin, db_exec, db_exec_teardown, is_credless, is_transient, nanos, Guard};

static LOCK: Mutex<()> = Mutex::new(());

fn run(args: &[&str]) -> Option<(bool, String, String)> {
    for attempt in 0..6 {
        let out = Command::new(bin()).args(args).output().expect("spawn travel");
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        if !out.status.success() && is_credless(&stderr) {
            return None;
        }
        if !out.status.success() && is_transient(&stderr) && attempt < 5 {
            std::thread::sleep(std::time::Duration::from_millis(400 * (attempt + 1)));
            continue;
        }
        return Some((out.status.success(), stdout, stderr));
    }
    unreachable!()
}

fn seed(run_id: &str) {
    let r = format!("'{run_id}'");
    db_exec(&format!(
        "INSERT INTO shaping_research_runs \
         (run_id, origin_code, pax, window_start, window_end, currency, exchange_rate_usd_twd, status, created_at, updated_at) \
         VALUES ({r}, 'TPE', 2, '2026-07-10', '2026-07-20', 'TWD', 32.0, 'ranked', '2026-07-01T00:00:00Z', '2026-07-01T00:00:00Z');"
    ))
    .expect("seed run");

    // Rules: preferred_sources (channel), flight_max_twd party cap (budget), exclude_hotel (lodging),
    // exclude_depart (date hard).
    db_exec(&format!(
        "INSERT INTO shaping_rules (run_id, aspect, role, kind, value_text, value_date, value_integer, notes, created_at) VALUES \
         ({r}, 'channel', 'soft_preference', 'preferred_sources', 'besttour, travel4u', NULL, NULL, 't', '2026-07-01T00:00:00Z'), \
         ({r}, 'budget', 'soft_preference', 'flight_max_twd', NULL, NULL, 16000, 't', '2026-07-01T00:00:00Z'), \
         ({r}, 'lodging', 'hard_constraint', 'exclude_hotel', '水之都那霸', NULL, NULL, 't', '2026-07-01T00:00:00Z'), \
         ({r}, 'date', 'hard_constraint', 'exclude_depart', NULL, '2026-07-15', NULL, 't', '2026-07-01T00:00:00Z');"
    ))
    .expect("seed rules");

    // One flight candidate: party total 18000 > 16000 cap → nudged down, NOT disqualified.
    db_exec(&format!(
        "INSERT INTO shaping_candidates (candidate_id, run_id, dest_code, depart_date, return_date, nights, flight_total_twd, leave_days, rank, verdict, adopted_plan_id) \
         VALUES ('{run_id}_cand', {r}, 'KIX', '2026-07-10', '2026-07-14', 4, 18000, 3, 1, 'k', NULL);"
    ))
    .expect("seed candidate");

    // Package offers:
    //  A: besttour (preferred), available, ok hotel → QUALIFIED, +2 channel
    //  B: eztravel (not preferred), available, ok hotel → QUALIFIED, -1 channel  (A must outrank B)
    //  C: besttour, SOLD_OUT → DISQUALIFIED (availability)
    //  D: besttour, available, excluded hotel 水之都那霸 → DISQUALIFIED (lodging)
    //  E: besttour, available, depart 2026-07-15 (excluded) → DISQUALIFIED (date)
    let ins = |sfx: &str, src: &str, depart: &str, hotel: &str, status: &str| {
        db_exec(&format!(
            "INSERT INTO shaping_tour_group_offers \
             (run_id, offer_id, source_id, dest_region, depart_date, return_date, nights, price_per_person_twd, title, url, scraped_at, hotel_name, departure_status, product_kind) \
             VALUES ({r}, '{run_id}_{sfx}', '{src}', 'kansai', '{depart}', '2026-07-14', 4, 20000, 'pkg {sfx}', 'http://x', '2026-07-01T00:00:00Z', '{hotel}', '{status}', 'group_tour');"
        )).expect("seed offer");
    };
    ins("A", "besttour", "2026-07-10", "Almont Naha", "available");
    ins("B", "eztravel", "2026-07-10", "Almont Naha", "available");
    ins("C", "besttour", "2026-07-10", "Almont Naha", "sold_out");
    ins("D", "besttour", "2026-07-10", "水之都那霸", "available");
    ins("E", "besttour", "2026-07-15", "Almont Naha", "available");
}

fn teardown(run_id: &str) {
    let r = format!("'{run_id}'");
    let _ = db_exec_teardown(&format!(
        "DELETE FROM shaping_tour_group_offers WHERE run_id = {r}; \
         DELETE FROM shaping_candidates WHERE run_id = {r}; \
         DELETE FROM shaping_rules WHERE run_id = {r}; \
         DELETE FROM shaping_research_runs WHERE run_id = {r};"
    ));
}

/// Return the single output line for an option id (e.g. "offer:<run>_A").
fn line_for<'a>(out: &'a str, opt_id: &str) -> Option<&'a str> {
    out.lines().find(|l| l.contains(opt_id))
}

#[test]
fn purchase_matrix_gates_nudges_sort_and_cost_scope() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if db_exec("SELECT 1 AS one").is_none() {
        eprintln!("skipping shaping-purchase-matrix test: no Turso credentials");
        return;
    }
    let run_id = format!("zz_pm_{}", nanos());
    // pre-clean any leftover, then arm the panic-safe guard BEFORE seeding.
    teardown(&run_id);
    let _g = Guard::new({
        let run_id = run_id.clone();
        move || teardown(&run_id)
    });
    seed(&run_id);

    // Full matrix (disqualified shown by default).
    let (ok, out, err) = run(&["shaping-purchase-matrix", "--run", &run_id]).expect("has creds");
    assert!(ok, "command should succeed; stderr={err}");

    // GATES → DISQUALIFIED with the specific reason.
    let c = line_for(&out, &format!("offer:{run_id}_C")).expect("C present");
    assert!(c.contains("DISQUALIFIED") && c.contains("FAIL_AVAIL"), "C sold_out disq: {c}");
    let d = line_for(&out, &format!("offer:{run_id}_D")).expect("D present");
    assert!(d.contains("DISQUALIFIED") && d.contains("FAIL_LODGING"), "D excluded-hotel disq: {d}");
    let e = line_for(&out, &format!("offer:{run_id}_E")).expect("E present");
    assert!(e.contains("DISQUALIFIED") && e.contains("FAIL_DATE"), "E excluded-depart disq: {e}");

    // NUDGE: preferred-source A (besttour) must score higher than non-preferred B (eztravel);
    // both QUALIFIED.
    let a = line_for(&out, &format!("offer:{run_id}_A")).expect("A present");
    let b = line_for(&out, &format!("offer:{run_id}_B")).expect("B present");
    assert!(a.contains("QUALIFIED"), "A qualified: {a}");
    assert!(b.contains("QUALIFIED"), "B qualified: {b}");
    // A appears before B in the output (higher score sorts first among qualified).
    let pos_a = out.find(&format!("offer:{run_id}_A")).unwrap();
    let pos_b = out.find(&format!("offer:{run_id}_B")).unwrap();
    assert!(pos_a < pos_b, "preferred-source A must sort before B");

    // Over-party-cap flight nudged down but NOT disqualified.
    let f = line_for(&out, &format!("flight:{run_id}_cand")).expect("flight present");
    assert!(f.contains("QUALIFIED"), "flight not disqualified: {f}");
    assert!(f.contains("over flight cap"), "flight nudged over cap: {f}");

    // COST_SCOPE labels present.
    assert!(f.contains("FLIGHT_ONLY"), "flight FLIGHT_ONLY: {f}");
    assert!(a.contains("PACKAGE_TOTAL"), "package PACKAGE_TOTAL: {a}");

    // DISQUALIFIED rows sort AFTER all qualified rows.
    let first_disq = out.find("DISQUALIFIED").unwrap();
    let last_qual = out.rfind(" QUALIFIED").unwrap_or_else(|| out.find("QUALIFIED").unwrap());
    assert!(last_qual < first_disq, "qualified rows must precede disqualified rows");

    // Plain text — no JSON.
    assert!(!out.contains('{') && !out.contains("\":"), "output must be plain text, not JSON");

    // --qualified-only hides the disqualified rows.
    let (ok2, out2, _) = run(&["shaping-purchase-matrix", "--run", &run_id, "--qualified-only"]).expect("creds");
    assert!(ok2);
    assert!(!out2.contains("DISQUALIFIED"), "--qualified-only hides disqualified rows");
    assert!(out2.contains(&format!("offer:{run_id}_A")), "qualified A still shown");
    assert!(!out2.contains(&format!("offer:{run_id}_C")), "sold_out C hidden");
}

#[test]
fn purchase_matrix_unknown_run_fails_loud() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if db_exec("SELECT 1 AS one").is_none() {
        eprintln!("skipping: no creds");
        return;
    }
    let (ok, _out, err) = run(&["shaping-purchase-matrix", "--run", "zz_nonexistent_run_xyz"]).expect("creds");
    assert!(!ok, "unknown run should fail");
    assert!(err.contains("not found"), "fail-loud msg: {err}");
}