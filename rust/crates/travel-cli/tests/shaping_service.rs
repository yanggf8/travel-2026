//! Integration test — port of tests/integration/shaping-service.regression.test.ts.
//!
//! Drives the built `travel` binary against a real Turso DB. Each case seeds
//! throwaway rows under a UNIQUE `shaping-test-<nanos>` run id (and, for the
//! adopt-to-new-plan case, a unique `shaping-test-plan-<nanos>` plan id), runs
//! the command under test, asserts, then tears everything down. Real plans
//! (tokyo-2026/kyoto-2026) and pre-existing runs are never touched.
//!
//! Skips cleanly (early return + eprintln) when Turso creds/connection are
//! absent so credless CI stays green.

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const BIN: &str = env!("CARGO_BIN_EXE_travel");

// ── credless skip detection ──────────────────────────────────────────

fn is_credless(stderr: &str) -> bool {
    stderr.contains("turso auth login")
        || stderr.contains("Missing Turso")
        || stderr.contains("failed to connect to Turso")
        || stderr.contains("TRAVEL_TURSO")
}

/// Transient network blips when many tests open Turso connections at once.
/// Retried (not treated as failures or credless skips).
fn is_transient(stderr: &str) -> bool {
    stderr.contains("Network is unreachable")
        || stderr.contains("error trying to connect")
        || stderr.contains("connection reset")
        || stderr.contains("connection closed")
}

/// Run the binary; returns (success, stdout, stderr). Returns None if the call
/// indicates missing credentials (caller should skip). Retries on transient
/// network errors.
fn run(args: &[&str]) -> Option<(bool, String, String)> {
    for attempt in 0..6 {
        let out = Command::new(BIN)
            .args(args)
            .output()
            .expect("spawn travel binary");
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

/// Run `shaping-init` with the given args, retrying on the second-granularity
/// run-id PK collision (shaping-YYYYMMDD-HHMMSS) that can happen when several
/// init-based tests run in parallel within the same wall-clock second. Returns
/// (stdout, created_run_id), or None when credless.
fn run_init(args: &[&str]) -> Option<(String, String)> {
    for _ in 0..30 {
        let (ok, stdout, stderr) = run(args)?;
        if ok {
            let created = stdout
                .lines()
                .find_map(|l| l.strip_prefix("Run created: "))
                .map(|s| s.trim().to_string())
                .expect("shaping-init prints 'Run created: <id>'");
            return Some((stdout, created));
        }
        if stderr.contains("UNIQUE constraint failed: shaping_research_runs.run_id") {
            std::thread::sleep(std::time::Duration::from_millis(1100));
            continue;
        }
        panic!("shaping-init failed: {stderr}");
    }
    panic!("shaping-init kept colliding on run id");
}

/// db exec helper. Returns plain-text stdout, or None when credless. Retries
/// on transient network errors.
fn db_exec(sql: &str) -> Option<String> {
    for attempt in 0..6 {
        let out = Command::new(BIN)
            .args(["db", "exec", sql])
            .output()
            .expect("spawn travel db exec");
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        if !out.status.success() && is_credless(&stderr) {
            return None;
        }
        if !out.status.success() && is_transient(&stderr) && attempt < 5 {
            std::thread::sleep(std::time::Duration::from_millis(400 * (attempt + 1)));
            continue;
        }
        if !out.status.success() {
            panic!("db exec failed: {sql}\n{stderr}");
        }
        return Some(String::from_utf8_lossy(&out.stdout).to_string());
    }
    unreachable!()
}

fn unique_run_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("shaping-test-{nanos}")
}

fn unique_plan_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("shaping-test-plan-{nanos}")
}

fn sql_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Tear down all rows for a throwaway run id.
fn cleanup_run(run_id: &str) {
    let r = sql_lit(run_id);
    let _ = db_exec(&format!(
        "DELETE FROM shaping_candidate_flights WHERE candidate_id IN (SELECT candidate_id FROM shaping_candidates WHERE run_id = {r}); \
         DELETE FROM shaping_candidates WHERE run_id = {r}; \
         DELETE FROM shaping_scrape_attempts WHERE run_id = {r}; \
         DELETE FROM shaping_rules WHERE run_id = {r}; \
         DELETE FROM shaping_research_durations WHERE run_id = {r}; \
         DELETE FROM shaping_research_destinations WHERE run_id = {r}; \
         DELETE FROM shaping_tour_group_offers WHERE run_id = {r}; \
         DELETE FROM shaping_research_runs WHERE run_id = {r};"
    ));
}

/// Tear down all rows for a throwaway plan id (mirrors deletePlanRows in TS).
fn cleanup_plan(plan_id: &str) {
    let p = sql_lit(plan_id);
    let _ = db_exec(&format!(
        "DELETE FROM plan_event_data WHERE plan_id = {p}; \
         DELETE FROM plan_events WHERE plan_id = {p}; \
         DELETE FROM event_log_next_actions WHERE plan_id = {p}; \
         DELETE FROM event_log_dest_processes WHERE plan_id = {p}; \
         DELETE FROM event_log_destinations WHERE plan_id = {p}; \
         DELETE FROM event_log_state WHERE plan_id = {p}; \
         DELETE FROM process_statuses WHERE plan_id = {p}; \
         DELETE FROM date_anchors WHERE plan_id = {p}; \
         DELETE FROM destination_cities WHERE plan_id = {p}; \
         DELETE FROM destination_details WHERE plan_id = {p}; \
         DELETE FROM plan_destinations WHERE plan_id = {p}; \
         DELETE FROM plan_metadata WHERE plan_id = {p}; \
         DELETE FROM plans WHERE plan_id = {p};"
    ));
}

/// Seed a research run row directly (status='started') so commands can resolve
/// it without going through shaping-init.
fn seed_run(run_id: &str) -> Option<()> {
    let r = sql_lit(run_id);
    db_exec(&format!(
        "INSERT INTO shaping_research_runs \
         (run_id, origin_code, pax, window_start, window_end, currency, exchange_rate_usd_twd, status, created_at, updated_at) \
         VALUES ({r}, 'TPE', 2, '2026-06-18', '2026-06-20', 'TWD', 32.0, 'started', '2026-06-01T00:00:00Z', '2026-06-01T00:00:00Z');"
    ))
    .map(|_| ())
}

/// Seed one candidate row directly.
fn seed_candidate(
    run_id: &str,
    candidate_id: &str,
    dest_code: &str,
    depart: &str,
    ret: &str,
    nights: i64,
    flight_total: i64,
    leave_days: i64,
) -> Option<()> {
    db_exec(&format!(
        "INSERT INTO shaping_candidates \
         (candidate_id, run_id, dest_code, depart_date, return_date, nights, flight_total_twd, leave_days, rank, verdict, adopted_plan_id) \
         VALUES ({}, {}, {}, {}, {}, {nights}, {flight_total}, {leave_days}, NULL, NULL, NULL);",
        sql_lit(candidate_id),
        sql_lit(run_id),
        sql_lit(dest_code),
        sql_lit(depart),
        sql_lit(ret),
    ))
    .map(|_| ())
}

// ── run creation ─────────────────────────────────────────────────────

#[tokio::test]
async fn creates_a_run_with_destinations_and_durations_reads_it_back() {
    // shaping-init generates its OWN run id (shaping-YYYYMMDD-HHMMSS); we cannot
    // pass a run id in. So drive shaping-init, capture the created run id from
    // stdout, then assert + clean THAT id up.
    let Some((_stdout, created)) = run_init(&[
        "shaping-init",
        "--origin",
        "TPE",
        "--start",
        "2026-06-18",
        "--end",
        "2026-06-20",
        "--dest",
        "KIX:Osaka/Kyoto (KIX)",
        "--dest",
        "NRT:Tokyo (NRT)",
        "--nights",
        "6",
        "--nights",
        "7",
        "--pax",
        "2",
    ]) else {
        eprintln!("skipping: no Turso credentials");
        return;
    };
    assert!(created.starts_with("shaping-"));

    // Read back the run.
    let row = db_exec(&format!(
        "SELECT origin_code, currency, status FROM shaping_research_runs WHERE run_id = {}",
        sql_lit(&created)
    ))
    .expect("read run");
    assert!(row.contains("origin_code: TPE"), "got: {row}");
    assert!(row.contains("currency: TWD"), "got: {row}");
    assert!(row.contains("status: started"), "got: {row}");

    // 2 destinations.
    let dests = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM shaping_research_destinations WHERE run_id = {}",
        sql_lit(&created)
    ))
    .unwrap();
    assert!(dests.contains("n: 2"), "expected 2 destinations, got: {dests}");

    // 2 durations; nights=6 → duration_days=7.
    let durs = db_exec(&format!(
        "SELECT duration_days FROM shaping_research_durations WHERE run_id = {} AND nights = 6",
        sql_lit(&created)
    ))
    .unwrap();
    assert!(durs.contains("duration_days: 7"), "got: {durs}");

    cleanup_run(&created);
}

#[tokio::test]
async fn creates_a_run_with_shaping_rules_and_reads_them_back() {
    let Some((_stdout, created)) = run_init(&[
        "shaping-init",
        "--origin",
        "TPE",
        "--start",
        "2026-06-12",
        "--end",
        "2026-06-25",
        "--dest",
        "OKA:Okinawa (OKA)",
        "--nights",
        "3",
        "--shaping",
        "date:hard_constraint:return_no_later_than:2026-06-27",
        "--shaping",
        "date:hard_constraint:exclude_depart:2026-06-28:Liko",
        "--shaping",
        "channel:hard_constraint:exclude_source:kkday",
        "--shaping",
        "mobility:hard_constraint:no_car:true",
    ]) else {
        eprintln!("skipping: no Turso credentials");
        return;
    };

    let count = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM shaping_rules WHERE run_id = {}",
        sql_lit(&created)
    ))
    .unwrap();
    assert!(count.contains("n: 4"), "expected 4 shaping rules, got: {count}");

    let has_kind = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM shaping_rules WHERE run_id = {} AND kind = 'return_no_later_than'",
        sql_lit(&created)
    ))
    .unwrap();
    assert!(has_kind.contains("n: 1"), "got: {has_kind}");

    cleanup_run(&created);
}

#[tokio::test]
async fn seeds_a_pending_scrape_attempt_per_destination_x_duration() {
    let Some((_stdout, created)) = run_init(&[
        "shaping-init",
        "--origin",
        "TPE",
        "--start",
        "2026-06-18",
        "--end",
        "2026-06-20",
        "--dest",
        "KIX:Osaka (KIX)",
        "--dest",
        "NRT:Tokyo (NRT)",
        "--nights",
        "6",
        "--nights",
        "7",
    ]) else {
        eprintln!("skipping: no Turso credentials");
        return;
    };

    // 2 destinations x 2 durations = 4 pending rows.
    let total = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM shaping_scrape_attempts WHERE run_id = {}",
        sql_lit(&created)
    ))
    .unwrap();
    assert!(total.contains("n: 4"), "expected 4 attempts, got: {total}");
    let pending = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM shaping_scrape_attempts WHERE run_id = {} AND status = 'pending'",
        sql_lit(&created)
    ))
    .unwrap();
    assert!(pending.contains("n: 4"), "expected 4 pending, got: {pending}");

    cleanup_run(&created);
}

// ── candidates and ranking ───────────────────────────────────────────

#[tokio::test]
async fn ranks_candidates_by_price_then_leave_days_then_depart_date() {
    let run_id = unique_run_id();
    if seed_run(&run_id).is_none() {
        eprintln!("skipping: no Turso credentials");
        return;
    }

    // Two candidates same price (18000) → leave-days breaks the tie; one cheaper
    // (15000) must rank first regardless of leave-days.
    let c1 = format!("{run_id}-KIX-2026-06-19-6n"); // 18000, leave 4
    let c2 = format!("{run_id}-KIX-2026-06-18-6n"); // 18000, leave 3
    let c3 = format!("{run_id}-KIX-2026-06-20-6n"); // 15000, leave 5 (cheapest)
    seed_candidate(&run_id, &c1, "KIX", "2026-06-19", "2026-06-25", 6, 18000, 4).unwrap();
    seed_candidate(&run_id, &c2, "KIX", "2026-06-18", "2026-06-24", 6, 18000, 3).unwrap();
    seed_candidate(&run_id, &c3, "KIX", "2026-06-20", "2026-06-26", 6, 15000, 5).unwrap();

    // Ranking is reachable only via shaping-import. A handoff file with NO
    // attempts (so no pair deletes) and NO new candidates leaves the seeded
    // rows untouched and ranks them. (Caveat: TS calls rankRun() directly; the
    // Rust ranking path is the same rank_run() invoked at the end of import.)
    let tmp = std::env::temp_dir().join(format!("{run_id}-import.json"));
    std::fs::write(&tmp, r#"{"candidates":[],"attempts":[]}"#).unwrap();
    let Some((ok, _so, se)) = run(&[
        "shaping-import",
        "--run",
        &run_id,
        "--file",
        tmp.to_str().unwrap(),
    ]) else {
        let _ = std::fs::remove_file(&tmp);
        cleanup_run(&run_id);
        eprintln!("skipping: no Turso credentials");
        return;
    };
    let _ = std::fs::remove_file(&tmp);
    assert!(ok, "shaping-import failed: {se}");

    // Read back ordered by rank.
    let rows = db_exec(&format!(
        "SELECT candidate_id, rank FROM shaping_candidates WHERE run_id = {} ORDER BY rank ASC",
        sql_lit(&run_id)
    ))
    .unwrap();
    let order: Vec<&str> = rows.lines().collect();
    assert_eq!(order.len(), 3, "expected 3 ranked rows, got: {rows}");
    assert!(order[0].contains(&c3) && order[0].contains("rank: 1"), "rank1: {}", order[0]);
    assert!(order[1].contains(&c2) && order[1].contains("rank: 2"), "rank2: {}", order[1]);
    assert!(order[2].contains(&c1) && order[2].contains("rank: 3"), "rank3: {}", order[2]);

    cleanup_run(&run_id);
}

#[tokio::test]
async fn records_scrape_attempts_and_reads_them_back() {
    // upsertScrapeAttempt has no dedicated CLI surface; the upsert is exercised
    // by shaping-import (INSERT OR REPLACE keyed on run+dest+nights). Seed an
    // import handoff whose attempts go pending → failed and confirm a SINGLE
    // row remains with the latest status/error.
    let run_id = unique_run_id();
    if seed_run(&run_id).is_none() {
        eprintln!("skipping: no Turso credentials");
        return;
    }

    // First import: one pending attempt (no candidates).
    let f1 = std::env::temp_dir().join(format!("{run_id}-a1.json"));
    std::fs::write(
        &f1,
        r#"{"candidates":[],"attempts":[{"destCode":"KIX","nights":6,"status":"pending"}]}"#,
    )
    .unwrap();
    // Second import: same pair now failed with an error.
    let f2 = std::env::temp_dir().join(format!("{run_id}-a2.json"));
    std::fs::write(
        &f2,
        r#"{"candidates":[],"attempts":[{"destCode":"KIX","nights":6,"status":"failed","error":"timeout"}]}"#,
    )
    .unwrap();

    let r1 = run(&["shaping-import", "--run", &run_id, "--file", f1.to_str().unwrap()]);
    let r2 = run(&["shaping-import", "--run", &run_id, "--file", f2.to_str().unwrap()]);
    let _ = std::fs::remove_file(&f1);
    let _ = std::fs::remove_file(&f2);
    let (Some(_), Some(_)) = (r1, r2) else {
        cleanup_run(&run_id);
        eprintln!("skipping: no Turso credentials");
        return;
    };

    let count = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM shaping_scrape_attempts WHERE run_id = {} AND dest_code = 'KIX' AND nights = 6",
        sql_lit(&run_id)
    ))
    .unwrap();
    assert!(count.contains("n: 1"), "expected exactly 1 attempt row, got: {count}");

    let row = db_exec(&format!(
        "SELECT status, error FROM shaping_scrape_attempts WHERE run_id = {} AND dest_code = 'KIX' AND nights = 6",
        sql_lit(&run_id)
    ))
    .unwrap();
    assert!(row.contains("status: failed"), "got: {row}");
    assert!(row.contains("error: timeout"), "got: {row}");

    cleanup_run(&run_id);
}

#[tokio::test]
async fn adopts_a_candidate_sets_adopted_plan_id_and_run_status() {
    let run_id = unique_run_id();
    if seed_run(&run_id).is_none() {
        eprintln!("skipping: no Turso credentials");
        return;
    }
    let candidate_id = format!("{run_id}-KIX-2026-06-18-6n");
    seed_candidate(&run_id, &candidate_id, "KIX", "2026-06-18", "2026-06-24", 6, 18000, 3).unwrap();

    // Adopt into an existing plan name (no --create-plan) — pointer-only update.
    let Some((ok, _so, se)) = run(&["shaping-adopt", &candidate_id, "osaka-2026"]) else {
        cleanup_run(&run_id);
        eprintln!("skipping: no Turso credentials");
        return;
    };
    assert!(ok, "shaping-adopt failed: {se}");

    let cand = db_exec(&format!(
        "SELECT adopted_plan_id FROM shaping_candidates WHERE candidate_id = {}",
        sql_lit(&candidate_id)
    ))
    .unwrap();
    assert!(cand.contains("adopted_plan_id: osaka-2026"), "got: {cand}");

    let run_row = db_exec(&format!(
        "SELECT status FROM shaping_research_runs WHERE run_id = {}",
        sql_lit(&run_id)
    ))
    .unwrap();
    assert!(run_row.contains("status: adopted"), "got: {run_row}");

    cleanup_run(&run_id);
}

#[tokio::test]
async fn delete_candidates_for_pair_clears_prior_candidates() {
    // deleteCandidatesForPair is exercised by shaping-import: a re-scrape of the
    // same pair that yields zero candidates must still clear the prior row (the
    // delete is keyed on the pair, listed in attempts, not the payload).
    let run_id = unique_run_id();
    if seed_run(&run_id).is_none() {
        eprintln!("skipping: no Turso credentials");
        return;
    }
    let candidate_id = format!("{run_id}-KIX-2026-06-18-6n");
    seed_candidate(&run_id, &candidate_id, "KIX", "2026-06-18", "2026-06-24", 6, 18000, 3).unwrap();
    // a candidate flight child row, to confirm cascade delete clears it too.
    db_exec(&format!(
        "INSERT INTO shaping_candidate_flights (candidate_id, direction, airline, depart_time, arrive_time, duration, nonstop, price_total_twd) \
         VALUES ({}, 'outbound', 'SL', '09:00', '12:30', '2h30m', 1, 9000);",
        sql_lit(&candidate_id)
    ))
    .unwrap();

    let before = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM shaping_candidates WHERE run_id = {}",
        sql_lit(&run_id)
    ))
    .unwrap();
    assert!(before.contains("n: 1"), "got: {before}");

    // Re-scrape of the KIX/6n pair with zero candidates → must clear the pair.
    let tmp = std::env::temp_dir().join(format!("{run_id}-clear.json"));
    std::fs::write(
        &tmp,
        r#"{"candidates":[],"attempts":[{"destCode":"KIX","nights":6,"status":"empty","candidateCount":0}]}"#,
    )
    .unwrap();
    let r = run(&["shaping-import", "--run", &run_id, "--file", tmp.to_str().unwrap()]);
    let _ = std::fs::remove_file(&tmp);
    if r.is_none() {
        cleanup_run(&run_id);
        eprintln!("skipping: no Turso credentials");
        return;
    }

    let after = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM shaping_candidates WHERE run_id = {}",
        sql_lit(&run_id)
    ))
    .unwrap();
    assert!(after.contains("n: 0"), "expected 0 after clear, got: {after}");
    let flights = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM shaping_candidate_flights WHERE candidate_id = {}",
        sql_lit(&candidate_id)
    ))
    .unwrap();
    assert!(flights.contains("n: 0"), "expected 0 flight rows, got: {flights}");

    cleanup_run(&run_id);
}

#[tokio::test]
async fn adopts_a_candidate_into_a_newly_seeded_plan_with_locked_p1_p2_rows() {
    let run_id = unique_run_id();
    let plan_id = unique_plan_id();
    if seed_run(&run_id).is_none() {
        eprintln!("skipping: no Turso credentials");
        return;
    }
    let candidate_id = format!("{run_id}-KIX-2026-06-18-6n");
    seed_candidate(&run_id, &candidate_id, "KIX", "2026-06-18", "2026-06-24", 6, 18000, 3).unwrap();

    // osaka_kyoto_2026 has KIX configured → adoption succeeds.
    let Some((ok, _so, se)) = run(&[
        "shaping-adopt",
        &candidate_id,
        &plan_id,
        "--create-plan",
        "--dest",
        "osaka_kyoto_2026",
    ]) else {
        cleanup_run(&run_id);
        eprintln!("skipping: no Turso credentials");
        return;
    };
    assert!(ok, "shaping-adopt --create-plan failed: {se}");

    // active_destination
    let meta = db_exec(&format!(
        "SELECT active_destination FROM plan_metadata WHERE plan_id = {}",
        sql_lit(&plan_id)
    ))
    .unwrap();
    assert!(meta.contains("active_destination: osaka_kyoto_2026"), "got: {meta}");

    // date anchor: start 2026-06-18, end 2026-06-24, days 7 (nights 6 + 1)
    let anchor = db_exec(&format!(
        "SELECT start_date, end_date, days FROM date_anchors WHERE plan_id = {} AND destination = 'osaka_kyoto_2026'",
        sql_lit(&plan_id)
    ))
    .unwrap();
    assert!(anchor.contains("start_date: 2026-06-18"), "got: {anchor}");
    assert!(anchor.contains("end_date: 2026-06-24"), "got: {anchor}");
    assert!(anchor.contains("days: 7"), "got: {anchor}");

    // process statuses: P1/P2 confirmed, rest pending
    let statuses = db_exec(&format!(
        "SELECT process_id, status FROM process_statuses WHERE plan_id = {} AND destination = 'osaka_kyoto_2026' ORDER BY process_id",
        sql_lit(&plan_id)
    ))
    .unwrap();
    assert!(statuses.contains("process_id: process_1_date_anchor, status: confirmed"), "got: {statuses}");
    assert!(statuses.contains("process_id: process_2_destination, status: confirmed"), "got: {statuses}");
    assert!(statuses.contains("process_id: process_3_4_packages, status: pending"), "got: {statuses}");
    assert!(statuses.contains("process_id: process_5_daily_itinerary, status: pending"), "got: {statuses}");

    // primary_airport = KIX
    let detail = db_exec(&format!(
        "SELECT primary_airport FROM destination_details WHERE plan_id = {} AND destination = 'osaka_kyoto_2026'",
        sql_lit(&plan_id)
    ))
    .unwrap();
    assert!(detail.contains("primary_airport: KIX"), "got: {detail}");

    // candidate pointer + run status
    let cand = db_exec(&format!(
        "SELECT adopted_plan_id FROM shaping_candidates WHERE candidate_id = {}",
        sql_lit(&candidate_id)
    ))
    .unwrap();
    assert!(cand.contains(&format!("adopted_plan_id: {plan_id}")), "got: {cand}");
    let run_row = db_exec(&format!(
        "SELECT status FROM shaping_research_runs WHERE run_id = {}",
        sql_lit(&run_id)
    ))
    .unwrap();
    assert!(run_row.contains("status: adopted"), "got: {run_row}");

    cleanup_plan(&plan_id);
    cleanup_run(&run_id);
}

#[tokio::test]
async fn rejects_create_plan_adoption_when_destination_airports_do_not_match() {
    let run_id = unique_run_id();
    let plan_id = unique_plan_id();
    if seed_run(&run_id).is_none() {
        eprintln!("skipping: no Turso credentials");
        return;
    }
    let candidate_id = format!("{run_id}-KIX-2026-06-18-6n");
    seed_candidate(&run_id, &candidate_id, "KIX", "2026-06-18", "2026-06-24", 6, 18000, 3).unwrap();

    // tokyo_2026 has NRT/HND, not KIX → must reject with a "does not match" error.
    let Some((ok, _so, stderr)) = run(&[
        "shaping-adopt",
        &candidate_id,
        &plan_id,
        "--create-plan",
        "--dest",
        "tokyo_2026",
    ]) else {
        cleanup_run(&run_id);
        eprintln!("skipping: no Turso credentials");
        return;
    };
    assert!(!ok, "expected failure, succeeded. stderr: {stderr}");
    assert!(
        stderr.contains("does not match destination tokyo_2026"),
        "expected mismatch error, got: {stderr}"
    );

    // No plan rows should have been created.
    let n = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM plan_metadata WHERE plan_id = {}",
        sql_lit(&plan_id)
    ))
    .unwrap();
    assert!(n.contains("n: 0"), "expected no plan_metadata rows, got: {n}");

    cleanup_plan(&plan_id);
    cleanup_run(&run_id);
}
