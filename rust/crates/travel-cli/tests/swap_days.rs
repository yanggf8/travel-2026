//! Real-Turso integration test for `travel swap-days <dayA> <dayB>`.
//!
//! This command had NO integration coverage despite being a content mutation
//! that swaps day-level themes and re-points every session-scoped row between
//! two day_numbers. The test also pins the audit-triad guarantee that the
//! shared `cascade::common::record_operation` helper enforces: every mutation
//! writes an `operation_runs` row AND bumps `plans.version`.
//!
//! Harness mirrors tests/soft_delete_plan.rs: seed a nanos-tagged throwaway
//! plan, run the binary, SELECT to assert, tear down. Skips cleanly when Turso
//! creds are absent. NEVER touches okinawa-2026 / tokyo-2026 / kyoto-2026.

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

/// Run `db exec`; Some((stdout,stderr)) on success, None on a credless skip,
/// panic on a real failure.
fn db_exec(sql: &str) -> Option<(String, String)> {
    let out = bin().args(["db", "exec", sql]).output().expect("run travel db exec");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if out.status.success() {
        return Some((stdout, stderr));
    }
    if is_credless(&stderr) {
        eprintln!("skipping swap-days Turso test: {}", stderr.trim());
        return None;
    }
    panic!("travel db exec failed: {}\nSQL: {sql}", stderr.trim());
}

fn run_cmd(args: &[&str]) -> (bool, String, String) {
    let out = bin()
        .args(args)
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// SELECT one scalar via db exec, parsing the `<col>: <value>` plain-text line.
fn scalar(sql: &str, col: &str) -> Option<String> {
    let (stdout, _) = db_exec(sql)?;
    let prefix = format!("{col}: ");
    Some(
        stdout
            .lines()
            .find_map(|l| l.strip_prefix(&prefix))
            .map(|s| s.trim().to_string())
            .unwrap_or_default(),
    )
}

/// Seed plan + metadata + two themed days (day 1 = arrival, day 2 = departure).
/// Returns false on a credless skip.
fn seed_two_days(plan_id: &str, dest: &str) -> bool {
    let sql = format!(
        "INSERT INTO plans (plan_id, schema_version, version) VALUES ('{plan_id}', '4.2.0', 0); \
         INSERT INTO plan_metadata (plan_id, schema_version, active_destination) \
           VALUES ('{plan_id}', '4.2.0', '{dest}'); \
         INSERT INTO days (plan_id, destination, day_number, date, theme, day_type, status) \
           VALUES ('{plan_id}', '{dest}', 1, '2026-09-01', 'THEME_ONE', 'arrival', 'draft'); \
         INSERT INTO days (plan_id, destination, day_number, date, theme, day_type, status) \
           VALUES ('{plan_id}', '{dest}', 2, '2026-09-02', 'THEME_TWO', 'departure', 'draft');"
    );
    db_exec(&sql).is_some()
}

fn teardown(plan_id: &str) {
    let sql = format!(
        "DELETE FROM plan_event_data WHERE plan_id = '{plan_id}'; \
         DELETE FROM plan_events WHERE plan_id = '{plan_id}'; \
         DELETE FROM operation_runs WHERE plan_id = '{plan_id}'; \
         DELETE FROM days WHERE plan_id = '{plan_id}'; \
         DELETE FROM plan_metadata WHERE plan_id = '{plan_id}'; \
         DELETE FROM plans WHERE plan_id = '{plan_id}';"
    );
    let _ = bin().args(["db", "exec", &sql]).output();
}

// ── swap-days swaps day themes AND fires the full audit triad ──────────────────
#[test]
fn swap_days_swaps_themes_and_writes_audit_triad() {
    let tag = nanos();
    let plan_id = format!("test-swapdays-{tag}");
    let dest = format!("swapdays_{tag}");
    if !seed_two_days(&plan_id, &dest) {
        return;
    }

    // Pre-conditions: themes as seeded, version starts at 0.
    let theme1_before = scalar(
        &format!("SELECT theme AS t FROM days WHERE plan_id='{plan_id}' AND day_number=1"),
        "t",
    );
    let v_before = scalar(
        &format!("SELECT version AS v FROM plans WHERE plan_id='{plan_id}'"),
        "v",
    );

    let (ok, stdout, stderr) =
        run_cmd(&["swap-days", "1", "2", "--plan-id", &plan_id, "--dest", &dest]);
    assert!(ok, "swap-days should succeed; stdout={stdout} stderr={stderr}");

    // Themes are now swapped: day 1 carries THEME_TWO, day 2 carries THEME_ONE.
    let theme1_after = scalar(
        &format!("SELECT theme AS t FROM days WHERE plan_id='{plan_id}' AND day_number=1"),
        "t",
    );
    let theme2_after = scalar(
        &format!("SELECT theme AS t FROM days WHERE plan_id='{plan_id}' AND day_number=2"),
        "t",
    );

    // Audit triad: version bumped 0 -> 1, an operation_runs row for swap-days,
    // and a `days_swapped` plan_event.
    let v_after = scalar(
        &format!("SELECT version AS v FROM plans WHERE plan_id='{plan_id}'"),
        "v",
    );
    let op_count = scalar(
        &format!(
            "SELECT COUNT(*) AS n FROM operation_runs \
             WHERE plan_id='{plan_id}' AND command_type='swap-days' AND status='completed'"
        ),
        "n",
    );
    // The mutation emits the `days_swapped` event in BOTH scopes
    // (dest_process + timeline), mirroring the canonical date-change cascade.
    let evt_count = scalar(
        &format!(
            "SELECT COUNT(*) AS n FROM plan_events \
             WHERE plan_id='{plan_id}' AND event='days_swapped'"
        ),
        "n",
    );
    let evt_scopes = scalar(
        &format!(
            "SELECT COUNT(DISTINCT scope) AS n FROM plan_events \
             WHERE plan_id='{plan_id}' AND event='days_swapped'"
        ),
        "n",
    );

    teardown(&plan_id);

    assert_eq!(theme1_before.as_deref(), Some("THEME_ONE"), "seed sanity");
    assert_eq!(v_before.as_deref(), Some("0"), "fresh plan starts at version 0");
    assert_eq!(
        theme1_after.as_deref(),
        Some("THEME_TWO"),
        "day 1 must take day 2's theme after swap"
    );
    assert_eq!(
        theme2_after.as_deref(),
        Some("THEME_ONE"),
        "day 2 must take day 1's theme after swap"
    );
    assert_eq!(v_after.as_deref(), Some("1"), "plans.version must bump to 1");
    assert_eq!(
        op_count.as_deref(),
        Some("1"),
        "exactly one completed operation_runs row for swap-days"
    );
    assert_eq!(
        evt_count.as_deref(),
        Some("2"),
        "days_swapped must be emitted once per scope (dest_process + timeline)"
    );
    assert_eq!(
        evt_scopes.as_deref(),
        Some("2"),
        "days_swapped events must span both the dest_process and timeline scopes"
    );
}

// ── swap-days rejects swapping a day onto itself (no write) ────────────────────
#[test]
fn swap_days_same_day_fails_loud() {
    // Cheap credless probe so we don't false-fail when creds are absent.
    if db_exec("SELECT 1 AS n").is_none() {
        return;
    }
    let plan_id = format!("test-swapdays-same-{}", nanos());
    let (ok, _stdout, stderr) = run_cmd(&["swap-days", "2", "2", "--plan-id", &plan_id]);
    assert!(!ok, "swapping a day onto itself must exit non-zero");
    assert!(
        stderr.contains("must be different"),
        "error must explain dayA/dayB must differ; got: {stderr}"
    );
}
