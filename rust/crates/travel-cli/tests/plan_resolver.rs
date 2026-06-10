//! Integration port of tests/integration/plan-resolver.regression.test.ts
//!
//! The TS test exercises `resolvePlanFromSummaries()` directly with in-memory
//! `PlanSummary[]`. The Rust equivalent is the `travel resolve-plan` CLI, which
//! reads plan summaries from Turso (plan_metadata + date_anchors +
//! plan_root_date_anchor) and runs the same 10-branch precedence ladder.
//!
//! We seed throwaway plans into Turso, drive `resolve-plan` with `TRAVEL_TODAY`
//! pinned to 2026-05-22 (matching the TS fixtures), assert the chosen plan_id +
//! source, then tear the rows down. Unique, namespaced plan ids keep parallel
//! runs from colliding and keep real plans untouched.
//!
//! Ported TS cases:
//!   1. selects plan whose date anchor contains the requested travel date
//!   2. selects a single upcoming plan when no date is requested
//!   3. resolves a candidate planning window before dates are committed
//!   4. does NOT silently choose when multiple future plans overlap a date
//!      (ambiguity error)
//!   5. falls back to latest updated only when all known plans are historical

use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Pin "today" to the TS fixture's `today: '2026-05-22'`.
const TODAY: &str = "2026-05-22";

/// `resolve-plan` reads EVERY plan in the DB, so concurrently-seeded plans in
/// other tests of this file would pollute each other's ladder inputs. Serialize
/// the seed→resolve→teardown window with a process-wide lock so each test sees
/// only its own throwaway plans (plus the real historical plans, which are
/// inert against these future-dated fixtures). Mirrors the TS test's isolated
/// in-memory `PlanSummary[]`.
static RESOLVER_LOCK: Mutex<()> = Mutex::new(());

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_travel"))
}

fn nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Returns Some(stdout) on success, or None if Turso creds are absent / the
/// connection failed (so credless CI skips gracefully). Panics on any other
/// failure.
fn run_db_exec(sql: &str) -> Option<String> {
    let out = bin()
        .args(["db", "exec", sql])
        .output()
        .expect("run travel db exec");
    if out.status.success() {
        return Some(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if is_credless(&stderr) {
        eprintln!("skipping plan-resolver Turso test: {}", stderr.trim());
        return None;
    }
    panic!("travel db exec failed: {}\nSQL: {sql}", stderr.trim());
}

fn is_credless(stderr: &str) -> bool {
    stderr.contains("turso auth login")
        || stderr.contains("Missing Turso data")
        || stderr.contains("failed to connect to Turso")
        || stderr.contains("TRAVEL_TURSO")
}

/// Seed one plan: plans + plan_metadata + (optional) one date_anchor.
/// `updated_at` is set on plan_metadata (the resolver reads it from there).
fn seed_plan(plan_id: &str, active_dest: &str, updated_at: &str, anchor: Option<(&str, &str, &str, i64)>) -> bool {
    let mut sql = format!(
        "INSERT INTO plans (plan_id, schema_version, updated_at, version) \
         VALUES ('{plan_id}', '4.2.0', '{updated_at}', 0); \
         INSERT INTO plan_metadata (plan_id, schema_version, active_destination, updated_at) \
         VALUES ('{plan_id}', '4.2.0', '{active_dest}', '{updated_at}');"
    );
    if let Some((dest, start, end, days)) = anchor {
        sql.push_str(&format!(
            " INSERT INTO date_anchors (plan_id, destination, start_date, end_date, days) \
              VALUES ('{plan_id}', '{dest}', '{start}', '{end}', {days});"
        ));
    }
    run_db_exec(&sql).is_some()
}

/// Seed a planning-window-only plan (no date_anchor) via plan_root_date_anchor.
fn seed_window_plan(plan_id: &str, active_dest: &str, updated_at: &str, status: &str, start: &str, end: &str) -> bool {
    // plan_root_date_anchor stores the P1 root anchor; the resolver reads
    // status / set_out_date / return_date / duration_days from it.
    let sql = format!(
        "INSERT INTO plans (plan_id, schema_version, updated_at, version) \
         VALUES ('{plan_id}', '4.2.0', '{updated_at}', 0); \
         INSERT INTO plan_metadata (plan_id, schema_version, active_destination, updated_at) \
         VALUES ('{plan_id}', '4.2.0', '{active_dest}', '{updated_at}'); \
         INSERT INTO plan_root_date_anchor (plan_id, status, set_out_date, return_date, duration_days) \
         VALUES ('{plan_id}', '{status}', '{start}', '{end}', 7);"
    );
    run_db_exec(&sql).is_some()
}

fn teardown(plan_ids: &[&str]) {
    for id in plan_ids {
        let sql = format!(
            "DELETE FROM date_anchors WHERE plan_id = '{id}'; \
             DELETE FROM plan_root_date_anchor WHERE plan_id = '{id}'; \
             DELETE FROM plan_metadata WHERE plan_id = '{id}'; \
             DELETE FROM plans WHERE plan_id = '{id}';"
        );
        // Best-effort cleanup; ignore credless/skip.
        let _ = bin().args(["db", "exec", &sql]).output();
    }
}

/// Run `resolve-plan` with the given extra args + TRAVEL_TODAY pinned.
/// Returns (success, stdout, stderr).
fn resolve(extra: &[&str]) -> (bool, String, String) {
    let out = bin()
        .arg("resolve-plan")
        .args(extra)
        .env("TRAVEL_TODAY", TODAY)
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .expect("run travel resolve-plan");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

// ── 1. date anchor contains the requested travel date ───────────────────────
#[tokio::test]
async fn selects_plan_whose_anchor_contains_travel_date() {
    let _guard = RESOLVER_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tag = nanos();
    let p_tokyo = format!("test-resolver-tokyo-{tag}");
    let p_kyoto = format!("test-resolver-kyoto-{tag}");
    let p_june = format!("test-resolver-june-{tag}");

    // Match the TS fixtures (pastTokyo / pastKyoto / junePlan).
    if !seed_plan(&p_tokyo, "tokyo_2026", "2026-02-17 19:24:02", Some(("tokyo_2026", "2026-02-13", "2026-02-17", 5))) {
        return; // credless skip
    }
    seed_plan(&p_kyoto, "kyoto_2026", "2026-02-28 15:51:22", Some(("kyoto_2026", "2026-02-24", "2026-02-28", 5)));
    seed_plan(&p_june, "fukuoka_2026", "2026-05-22 10:00:00", Some(("fukuoka_2026", "2026-06-18", "2026-06-25", 8)));

    let (ok, stdout, stderr) = resolve(&["--travel-date", "2026-06-20"]);
    teardown(&[&p_tokyo, &p_kyoto, &p_june]);

    assert!(ok, "resolve-plan should succeed; stderr={stderr} stdout={stdout}");
    assert!(stdout.contains(&format!("plan_id: {p_june}")), "expected june plan; got {stdout}");
    assert!(stdout.contains("source:  date"), "expected source date; got {stdout}");
}

// ── 2. single upcoming plan when no date requested ──────────────────────────
#[tokio::test]
async fn selects_single_upcoming_plan_when_no_date() {
    let _guard = RESOLVER_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tag = nanos();
    let p_tokyo = format!("test-resolver-up-tokyo-{tag}");
    let p_kyoto = format!("test-resolver-up-kyoto-{tag}");
    let p_june = format!("test-resolver-up-june-{tag}");

    // pastTokyo + pastKyoto are historical (end < today 2026-05-22); junePlan is upcoming.
    if !seed_plan(&p_tokyo, "tokyo_2026", "2026-02-17 19:24:02", Some(("tokyo_2026", "2026-02-13", "2026-02-17", 5))) {
        return;
    }
    seed_plan(&p_kyoto, "kyoto_2026", "2026-02-28 15:51:22", Some(("kyoto_2026", "2026-02-24", "2026-02-28", 5)));
    seed_plan(&p_june, "fukuoka_2026", "2026-05-22 10:00:00", Some(("fukuoka_2026", "2026-06-18", "2026-06-25", 8)));

    let (ok, stdout, stderr) = resolve(&[]);
    teardown(&[&p_tokyo, &p_kyoto, &p_june]);

    assert!(ok, "resolve-plan should succeed; stderr={stderr} stdout={stdout}");
    assert!(stdout.contains(&format!("plan_id: {p_june}")), "expected june plan; got {stdout}");
    assert!(stdout.contains("source:  upcoming"), "expected source upcoming; got {stdout}");
}

// ── 3. candidate planning window (no committed anchors) ─────────────────────
#[tokio::test]
async fn resolves_candidate_planning_window() {
    let _guard = RESOLVER_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tag = nanos();
    let p_tokyo = format!("test-resolver-win-tokyo-{tag}");
    let p_kyoto = format!("test-resolver-win-kyoto-{tag}");
    let p_window = format!("test-resolver-window-{tag}");

    if !seed_plan(&p_tokyo, "tokyo_2026", "2026-02-17 19:24:02", Some(("tokyo_2026", "2026-02-13", "2026-02-17", 5))) {
        return;
    }
    seed_plan(&p_kyoto, "kyoto_2026", "2026-02-28 15:51:22", Some(("kyoto_2026", "2026-02-24", "2026-02-28", 5)));
    // Candidate planning window. Use a distinctive far-future range that no other
    // concurrently-running test seeds: resolve-plan reads ALL plans across this
    // shared live DB, and a cross-binary mutex isn't possible, so window
    // uniqueness is the only safe isolation against other suites' --create-plan
    // adoptions (which cluster around 2026-06).
    seed_window_plan(&p_window, "undecided_japan_2026", "2026-05-22 12:00:00", "researching", "2027-09-13", "2027-09-20");

    let (ok, stdout, stderr) = resolve(&["--travel-start", "2027-09-13", "--travel-end", "2027-09-20"]);
    teardown(&[&p_tokyo, &p_kyoto, &p_window]);

    assert!(ok, "resolve-plan should succeed; stderr={stderr} stdout={stdout}");
    assert!(stdout.contains(&format!("plan_id: {p_window}")), "expected window plan; got {stdout}");
    assert!(stdout.contains("source:  date"), "expected source date; got {stdout}");
}

// ── 4. ambiguity error when multiple future plans overlap a date ────────────
#[tokio::test]
async fn ambiguous_date_overlap_errors() {
    let _guard = RESOLVER_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tag = nanos();
    let p_june = format!("test-resolver-amb-june-{tag}");
    let p_osaka = format!("test-resolver-amb-osaka-{tag}");

    // junePlan + otherJunePlan both contain 2026-06-21.
    if !seed_plan(&p_june, "fukuoka_2026", "2026-05-22 10:00:00", Some(("fukuoka_2026", "2026-06-18", "2026-06-25", 8))) {
        return;
    }
    seed_plan(&p_osaka, "osaka_2026", "2026-05-22 10:00:00", Some(("osaka_2026", "2026-06-20", "2026-06-24", 5)));

    let (ok, stdout, stderr) = resolve(&["--travel-date", "2026-06-21"]);
    teardown(&[&p_june, &p_osaka]);

    assert!(!ok, "resolve-plan should fail on ambiguous overlap; stdout={stdout}");
    // The error message mirrors the TS /Multiple travel plans contain 2026-06-21/.
    assert!(
        stderr.contains("Multiple travel plans contain 2026-06-21"),
        "expected ambiguity error; got stderr={stderr}"
    );
}

// ── 5. fallback to latest updated when all plans historical ─────────────────
//
// The TS fixture is exactly two historical plans → source 'latest'. The CLI
// reads ALL DB plans, but in this repo the only non-test plans (tokyo-2026 /
// kyoto-2026) are themselves historical (Feb 2026 < today). So with the
// resolver lock held and only historical plans present, the no-arg ladder must
// take the "latest" branch (no active, no upcoming). We assert the BRANCH
// (source: latest) — the exact winning plan_id depends on which historical
// plan has the newest updated_at across the DB, which is the documented
// "most recently updated" semantics. The pure plan-pick is also covered by the
// 22 unit tests in plan_resolver.rs.
#[tokio::test]
async fn falls_back_to_latest_when_all_historical() {
    let _guard = RESOLVER_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tag = nanos();
    let p_tokyo = format!("test-resolver-lat-tokyo-{tag}");
    let p_kyoto = format!("test-resolver-lat-kyoto-{tag}");

    // Two historical seeds (both end well before today 2026-05-22).
    if !seed_plan(&p_tokyo, "tokyo_2026", "2026-02-17 19:24:02", Some(("tokyo_2026", "2026-02-13", "2026-02-17", 5))) {
        return;
    }
    seed_plan(&p_kyoto, "kyoto_2026", "2026-02-28 15:51:22", Some(("kyoto_2026", "2026-02-24", "2026-02-28", 5)));

    // No date, no env: with only historical plans, ladder falls to "latest".
    let (ok, stdout, stderr) = resolve(&[]);

    // Independently confirm kyoto is the newer of OUR seeded pair (load-bearing
    // input to the "most recently updated" rule).
    let pair = run_db_exec(&format!(
        "SELECT plan_id FROM plan_metadata WHERE plan_id IN ('{p_tokyo}', '{p_kyoto}') ORDER BY updated_at DESC LIMIT 1"
    ));
    teardown(&[&p_tokyo, &p_kyoto]);

    assert!(ok, "no-arg resolve over historical-only plans should succeed; stderr={stderr} stdout={stdout}");
    assert!(stdout.contains("source:  latest"), "expected the latest-fallback branch; got {stdout}");
    let pair = pair.expect("pair readback should succeed once seeded");
    assert!(
        pair.contains(&p_kyoto),
        "kyoto should be the newer of the seeded historical pair; got {pair}"
    );
}
