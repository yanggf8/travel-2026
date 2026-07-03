//! Real-Turso behavior LOCK for `shaping-init`, capturing its FULL write surface
//! before the DAL migration (Unit 1: run_init → repo::shaping). It must PASS
//! against the CURRENT (un-migrated) code; the migration must keep it green.
//!
//! shaping-init writes five tables (no plan, no operation_runs): shaping_research_runs,
//! shaping_research_destinations, shaping_research_durations, shaping_rules, and one
//! 'pending' shaping_scrape_attempts row per (dest × duration). run_init mints its OWN
//! run id (shaping-YYYYMMDD-HHMMSS) and prints it as "Run created: <id>" — we drive the
//! real binary and capture that id, then assert every written row scoped to it.
//!
//! Idioms mirror tests/shaping_service.rs (credless skip, transient retry, run-id
//! PK-collision retry, unique-per-run teardown). Skips cleanly without Turso creds.

use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{bin, db_exec, db_exec_teardown, is_credless, is_transient, Guard};

// Serialize this file's cases (each mints a second-granularity run id).
static INIT_LOCK: Mutex<()> = Mutex::new(());

/// Run the binary; None ⇒ credless (skip). Retries transient network errors.
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

/// Drive shaping-init, retrying the second-granularity run-id PK collision. Returns
/// the created run id, or None when credless.
fn run_init(args: &[&str]) -> Option<String> {
    for _ in 0..30 {
        let (ok, stdout, stderr) = run(args)?;
        if ok {
            let created = stdout
                .lines()
                .find_map(|l| l.strip_prefix("Run created: "))
                .map(|s| s.trim().to_string())
                .expect("shaping-init prints 'Run created: <id>'");
            return Some(created);
        }
        if stderr.contains("UNIQUE constraint failed: shaping_research_runs.run_id") {
            std::thread::sleep(std::time::Duration::from_millis(1100));
            continue;
        }
        panic!("shaping-init failed: {stderr}");
    }
    panic!("shaping-init kept colliding on run id");
}

fn sql_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn cleanup_run(run_id: &str) {
    let r = sql_lit(run_id);
    let _ = db_exec_teardown(&format!(
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

#[test]
fn shaping_init_writes_full_research_surface() {
    let _lock = INIT_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    // Credless probe: if db exec can't reach Turso, skip cleanly.
    if db_exec("SELECT 1 AS n").is_none() {
        eprintln!("skipping shaping-init lock (no Turso creds)");
        return;
    }

    // Drive the real command: 2 destinations, 2 durations, 1 typed shaping rule.
    let Some(run_id) = run_init(&[
        "shaping-init",
        "--origin",
        "tpe",
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
        "--pax",
        "3",
        "--rate",
        "31.5",
        // aspect:role:kind:value[:notes] — an integer-valued rule.
        "--shaping",
        "budget:max:integer:60000:cap per person",
    ]) else {
        eprintln!("skipping shaping-init lock (no Turso creds)");
        return;
    };
    let _g = Guard::new({
        let run_id = run_id.clone();
        move || cleanup_run(&run_id)
    });
    let r = sql_lit(&run_id);

    // --- shaping_research_runs: the parent row (origin uppercased, currency TWD, status started) ---
    let run_row = db_exec(&format!(
        "SELECT origin_code || '|' || pax || '|' || window_start || '|' || window_end || '|' || \
                currency || '|' || exchange_rate_usd_twd || '|' || status AS v \
         FROM shaping_research_runs WHERE run_id = {r}"
    ))
    .unwrap();
    assert_eq!(
        run_row.scalar().as_deref(),
        Some("TPE|3|2026-06-18|2026-06-20|TWD|31.5|started"),
        "shaping_research_runs parent row; out={run_row}"
    );

    // --- shaping_research_destinations: in --dest order, sort_order 0..n ---
    let dests = db_exec(&format!(
        "SELECT sort_order || ':' || dest_code || ':' || dest_label AS v \
         FROM shaping_research_destinations WHERE run_id = {r} ORDER BY sort_order"
    ))
    .unwrap();
    assert_eq!(
        dests.column(),
        vec!["0:KIX:Osaka (KIX)", "1:NRT:Tokyo (NRT)"],
        "destinations persist in --dest order; out={dests}"
    );

    // --- shaping_research_durations: nights + duration_days = nights + 1 ---
    let durs = db_exec(&format!(
        "SELECT nights || ':' || duration_days AS v \
         FROM shaping_research_durations WHERE run_id = {r} ORDER BY nights"
    ))
    .unwrap();
    assert_eq!(
        durs.column(),
        vec!["6:7", "7:8"],
        "durations persist with duration_days = nights + 1; out={durs}"
    );

    // --- shaping_rules: the typed rule (integer value → value_integer, text/date NULL) ---
    let rule = db_exec(&format!(
        "SELECT aspect || '|' || role || '|' || kind || '|' || \
                COALESCE(value_text, 'NULL') || '|' || COALESCE(value_date, 'NULL') || '|' || \
                COALESCE(value_integer, -1) || '|' || COALESCE(notes, 'NULL') AS v \
         FROM shaping_rules WHERE run_id = {r}"
    ))
    .unwrap();
    assert_eq!(
        rule.scalar().as_deref(),
        Some("budget|max|integer|NULL|NULL|60000|cap per person"),
        "typed shaping rule persists into value_integer (text/date NULL); out={rule}"
    );

    // --- shaping_scrape_attempts: one 'pending' row per (dest × duration), NULL fields ---
    let attempts_count = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM shaping_scrape_attempts WHERE run_id = {r}"
    ))
    .unwrap();
    assert_eq!(
        attempts_count.scalar().as_deref(),
        Some("4"),
        "one scrape attempt per (2 dests × 2 durations) = 4; out={attempts_count}"
    );
    let attempts = db_exec(&format!(
        "SELECT dest_code || ':' || nights || ':' || status || ':' || \
                COALESCE(candidate_count, -1) || ':' || COALESCE(error, 'NULL') || ':' || \
                COALESCE(attempted_at, 'NULL') AS v \
         FROM shaping_scrape_attempts WHERE run_id = {r} ORDER BY dest_code, nights"
    ))
    .unwrap();
    assert_eq!(
        attempts.column(),
        vec![
            "KIX:6:pending:-1:NULL:NULL",
            "KIX:7:pending:-1:NULL:NULL",
            "NRT:6:pending:-1:NULL:NULL",
            "NRT:7:pending:-1:NULL:NULL",
        ],
        "pending scrape attempts seeded per (dest × duration) with NULL count/error/attempted_at; out={attempts}"
    );

    // shaping-init creates NO plan and NO operation_runs.
    // (Guard's Drop tears down every shaping_* row for this run id, on return or panic.)
}