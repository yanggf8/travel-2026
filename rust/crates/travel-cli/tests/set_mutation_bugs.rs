//! Regression tests for the silent-no-op + `--dest`-parsing bugs in the
//! `set-*` mutation commands (see docs/handoff-cli-mutation-bugs.md).
//!
//! Two distinct invariants are exercised here:
//!
//! Bug 1 — a `completed`/`✅` audit MUST imply a domain row actually changed.
//!   - `set-flight` / `set-hotel` are booking-first: they must PERSIST on a
//!     fresh plan that never had an offer-cascade skeleton row (upsert).
//!   - `set-day-theme` / `set-tod-focus` / `set-route-segment` / `set-activity`
//!     require their parent day/session/activity row to pre-exist: on a missing
//!     parent they must exit non-zero and write NO `completed` operation_runs
//!     row.
//!
//! Bug 2 — `--dest <slug>` is advertised in the Example and must be accepted by
//!   `set-flight` / `set-hotel` / `set-airport-transfer` (previously the
//!   catch-all `--` arm in `parse_args` rejected it).
//!
//! Pattern mirrors state_manager.rs: seed a throwaway plan, run the binary,
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
        eprintln!("skipping set-mutation Turso test: {}", stderr.trim());
        return None;
    }
    panic!("travel db exec failed: {}\nSQL: {sql}", stderr.trim());
}

/// Seed only `plans` + `plan_metadata` — a hand-scaffolded plan with NO
/// flight_legs / hotels / days / timesofday / activities skeleton rows.
/// Returns false on a credless skip.
fn seed_bare(plan_id: &str, dest: &str) -> bool {
    let sql = format!(
        "INSERT INTO plans (plan_id, schema_version, version) VALUES ('{plan_id}', '4.2.0', 0); \
         INSERT INTO plan_metadata (plan_id, schema_version, active_destination) \
           VALUES ('{plan_id}', '4.2.0', '{dest}');"
    );
    db_exec(&sql).is_some()
}

fn teardown(plan_id: &str) {
    let sql = format!(
        "DELETE FROM plan_event_data WHERE plan_id = '{plan_id}'; \
         DELETE FROM plan_events WHERE plan_id = '{plan_id}'; \
         DELETE FROM operation_runs WHERE plan_id = '{plan_id}'; \
         DELETE FROM hotel_access_lines WHERE plan_id = '{plan_id}'; \
         DELETE FROM hotels WHERE plan_id = '{plan_id}'; \
         DELETE FROM airport_transfer_candidates WHERE plan_id = '{plan_id}'; \
         DELETE FROM airport_transfers WHERE plan_id = '{plan_id}'; \
         DELETE FROM flight_legs WHERE plan_id = '{plan_id}'; \
         DELETE FROM day_route_segments WHERE plan_id = '{plan_id}'; \
         DELETE FROM activities WHERE plan_id = '{plan_id}'; \
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
    // db exec renders `SELECT COUNT(*) AS n ...` as `n: <count>`.
    let n = out
        .lines()
        .find_map(|l| l.strip_prefix("n: "))
        .map(|s| s.trim().parse::<i64>().unwrap_or(-1))
        .unwrap_or(0);
    Some(n)
}

// ── Bug 1: set-flight upserts on a fresh plan (no skeleton row) ──────────────
#[test]
fn set_flight_persists_on_fresh_plan() {
    let tag = nanos();
    let plan_id = format!("test-setbug-flight-{tag}");
    let dest = format!("setbugflight_{tag}");
    if !seed_bare(&plan_id, &dest) {
        return;
    }

    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &[
            "set-flight", "outbound", "--dest", &dest,
            "--flight", "CI120", "--airline", "China Airlines", "--airline-code", "CI",
            "--from", "TPE", "--dep", "08:00", "--to", "OKA", "--arr", "10:45",
            "--date", "2026-06-12",
        ],
    );

    let row_count = count(&format!(
        "SELECT COUNT(*) AS n FROM flight_legs WHERE plan_id = '{plan_id}' AND direction = 'outbound'"
    ));
    let flight = db_exec(&format!(
        "SELECT flight_number FROM flight_legs WHERE plan_id = '{plan_id}' AND direction = 'outbound'"
    ));
    teardown(&plan_id);

    assert!(ok, "set-flight should succeed on a fresh plan; stdout={stdout} stderr={stderr}");
    assert_eq!(row_count, Some(1), "an outbound flight_legs row must be persisted");
    assert!(
        flight.unwrap_or_default().contains("flight_number: CI120"),
        "the persisted leg must carry the flight number"
    );
}

// ── Bug 1: set-hotel upserts on a fresh plan (no skeleton row) ───────────────
#[test]
fn set_hotel_persists_on_fresh_plan() {
    let tag = nanos();
    let plan_id = format!("test-setbug-hotel-{tag}");
    let dest = format!("setbughotel_{tag}");
    if !seed_bare(&plan_id, &dest) {
        return;
    }

    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &[
            "set-hotel", "--dest", &dest,
            "--name", "Hotel Aqua Citta Naha", "--check-in", "2026-06-21",
            "--access", "Yui Rail Miebashi 5min",
        ],
    );

    let row_count = count(&format!(
        "SELECT COUNT(*) AS n FROM hotels WHERE plan_id = '{plan_id}'"
    ));
    let name = db_exec(&format!(
        "SELECT name FROM hotels WHERE plan_id = '{plan_id}'"
    ));
    let access_count = count(&format!(
        "SELECT COUNT(*) AS n FROM hotel_access_lines WHERE plan_id = '{plan_id}'"
    ));
    teardown(&plan_id);

    assert!(ok, "set-hotel should succeed on a fresh plan; stdout={stdout} stderr={stderr}");
    assert_eq!(row_count, Some(1), "a hotels row must be persisted");
    assert!(
        name.unwrap_or_default().contains("name: Hotel Aqua Citta Naha"),
        "the persisted hotel must carry the name"
    );
    assert_eq!(access_count, Some(1), "the access line must be persisted");
}

// ── Bug 2: set-flight accepts --dest (advertised in the Example) ─────────────
// (covered by set_flight_persists_on_fresh_plan, which passes --dest; this
//  asserts the parser specifically does not reject it.)
#[test]
fn set_flight_accepts_dest_flag() {
    let tag = nanos();
    let plan_id = format!("test-setbug-fdest-{tag}");
    let dest = format!("setbugfdest_{tag}");
    if !seed_bare(&plan_id, &dest) {
        return;
    }

    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &[
            "set-flight", "outbound", "--dest", &dest,
            "--flight", "CI120", "--from", "TPE", "--to", "OKA",
        ],
    );
    teardown(&plan_id);

    assert!(
        ok && !stderr.contains("unknown argument: --dest"),
        "set-flight must accept --dest; stdout={stdout} stderr={stderr}"
    );
}

// ── Bug 1: set-airport-transfer upserts on a fresh plan (no skeleton row) ────
// Also exercises Bug 2: --dest must be accepted by set-airport-transfer.
#[test]
fn set_airport_transfer_persists_on_fresh_plan() {
    let tag = nanos();
    let plan_id = format!("test-setbug-xfer-{tag}");
    let dest = format!("setbugxfer_{tag}");
    if !seed_bare(&plan_id, &dest) {
        return;
    }

    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &[
            "set-airport-transfer", "arrival", "planned", "--dest", &dest,
            "--selected", "Yui Rail|OKA → Miebashi|14|340|10:55 → 11:09",
        ],
    );

    let row_count = count(&format!(
        "SELECT COUNT(*) AS n FROM airport_transfers \
         WHERE plan_id = '{plan_id}' AND direction = 'arrival'"
    ));
    let title = db_exec(&format!(
        "SELECT selected_title FROM airport_transfers \
         WHERE plan_id = '{plan_id}' AND direction = 'arrival'"
    ));
    teardown(&plan_id);

    assert!(
        ok && !stderr.contains("unknown argument: --dest"),
        "set-airport-transfer should succeed (and accept --dest) on a fresh plan; stdout={stdout} stderr={stderr}"
    );
    assert_eq!(
        row_count,
        Some(1),
        "an arrival airport_transfers row must be persisted"
    );
    assert!(
        title.unwrap_or_default().contains("selected_title: Yui Rail"),
        "the persisted transfer must carry the selected title"
    );
}

// ── Bug 1 fail-loud: set-day-theme on a missing day → no completed audit ─────
#[test]
fn set_day_theme_fails_loud_when_day_missing() {
    let tag = nanos();
    let plan_id = format!("test-setbug-theme-{tag}");
    let dest = format!("setbugtheme_{tag}");
    if !seed_bare(&plan_id, &dest) {
        return;
    }

    // No `days` row exists for day 1.
    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &["set-day-theme", "1", "Some theme", "--dest", &dest],
    );

    let audit = count(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs \
         WHERE plan_id = '{plan_id}' AND command_type = 'set-day-theme' AND status = 'completed'"
    ));
    teardown(&plan_id);

    assert!(
        !ok,
        "set-day-theme must exit non-zero when the day row is missing; stdout={stdout} stderr={stderr}"
    );
    assert_eq!(
        audit,
        Some(0),
        "no completed set-day-theme audit row may be written when nothing changed"
    );
}

// ── Bug 1 fail-loud: set-tod-focus on a missing session → no completed audit ─
#[test]
fn set_tod_focus_fails_loud_when_session_missing() {
    let tag = nanos();
    let plan_id = format!("test-setbug-tod-{tag}");
    let dest = format!("setbugtod_{tag}");
    if !seed_bare(&plan_id, &dest) {
        return;
    }

    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &["set-tod-focus", "1", "morning", "Explore Naha", "--dest", &dest],
    );

    let audit = count(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs \
         WHERE plan_id = '{plan_id}' AND command_type = 'set-tod-focus' AND status = 'completed'"
    ));
    teardown(&plan_id);

    assert!(
        !ok,
        "set-tod-focus must exit non-zero when the session row is missing; stdout={stdout} stderr={stderr}"
    );
    assert_eq!(
        audit,
        Some(0),
        "no completed set-tod-focus audit row may be written when nothing changed"
    );
}

// ── Bug 1 fail-loud: set-route-segment on a missing day → no completed audit ─
#[test]
fn set_route_segment_fails_loud_when_day_missing() {
    let tag = nanos();
    let plan_id = format!("test-setbug-route-{tag}");
    let dest = format!("setbugroute_{tag}");
    if !seed_bare(&plan_id, &dest) {
        return;
    }

    // Positional syntax: <day> <sort_order> <from> <to> <mode> [flags].
    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &[
            "set-route-segment", "1", "0", "Naha Airport", "Hotel", "transit",
            "--dest", &dest,
        ],
    );

    let audit = count(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs \
         WHERE plan_id = '{plan_id}' AND command_type = 'set-route-segment' AND status = 'completed'"
    ));
    let seg = count(&format!(
        "SELECT COUNT(*) AS n FROM day_route_segments WHERE plan_id = '{plan_id}'"
    ));
    teardown(&plan_id);

    assert!(
        !ok,
        "set-route-segment must exit non-zero when the day row is missing; stdout={stdout} stderr={stderr}"
    );
    assert_eq!(
        audit,
        Some(0),
        "no completed set-route-segment audit row may be written when the parent day is missing"
    );
    assert_eq!(
        seg,
        Some(0),
        "no orphan day_route_segments row may be written when the parent day is missing"
    );
}

// ── Bug 1 fail-loud: set-activity-time on a missing activity → no audit ──────
#[test]
fn set_activity_time_fails_loud_when_activity_missing() {
    let tag = nanos();
    let plan_id = format!("test-setbug-act-{tag}");
    let dest = format!("setbugact_{tag}");
    if !seed_bare(&plan_id, &dest) {
        return;
    }

    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &["set-activity-time", "1", "morning", "checkout", "--start", "11:00", "--dest", &dest],
    );

    let audit = count(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs \
         WHERE plan_id = '{plan_id}' AND command_type = 'set-activity-time' AND status = 'completed'"
    ));
    teardown(&plan_id);

    assert!(
        !ok,
        "set-activity-time must exit non-zero when no matching activity exists; stdout={stdout} stderr={stderr}"
    );
    assert_eq!(
        audit,
        Some(0),
        "no completed set-activity-time audit row may be written when nothing matched"
    );
}
