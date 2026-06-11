//! Real-Turso integration tests for the map-snapshot staleness lint:
//! `mark-maps-snapshotted` (records snapshot time) + `check-maps-fresh` (the
//! lint). Pattern mirrors set_mutation_bugs.rs: seed a throwaway plan, run the
//! binary, assert on output, tear down. Skips cleanly when Turso creds absent.
//!
//! NEVER touches okinawa-2026 / tokyo-2026 / kyoto-2026 — all plan ids here are
//! `test-mapsfresh-*` + a nanosecond tag, removed in teardown.

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
        eprintln!("skipping maps-fresh Turso test: {}", stderr.trim());
        return None;
    }
    panic!("travel db exec failed: {}\nSQL: {sql}", stderr.trim());
}

/// Seed a throwaway plan with one activity row (so MAX(updated_at) has something
/// to compare). Returns false on a credless skip.
fn seed(plan_id: &str, dest: &str) -> bool {
    let sql = format!(
        "INSERT INTO plans (plan_id, schema_version, version) VALUES ('{plan_id}', '4.2.0', 0); \
         INSERT INTO plan_metadata (plan_id, schema_version, active_destination) \
           VALUES ('{plan_id}', '4.2.0', '{dest}'); \
         INSERT INTO days (plan_id, destination, day_number, date, day_type, updated_at) \
           VALUES ('{plan_id}', '{dest}', 1, '2026-06-12', 'arrival', datetime('now')); \
         INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title, updated_at) \
           VALUES ('{plan_id}-act1', '{plan_id}', '{dest}', 1, 'morning', 0, 'Seed activity', datetime('now'));"
    );
    db_exec(&sql).is_some()
}

fn teardown(plan_id: &str) {
    let sql = format!(
        "DELETE FROM plan_map_snapshots WHERE plan_id = '{plan_id}'; \
         DELETE FROM activities WHERE plan_id = '{plan_id}'; \
         DELETE FROM session_meals WHERE plan_id = '{plan_id}'; \
         DELETE FROM timesofday WHERE plan_id = '{plan_id}'; \
         DELETE FROM days WHERE plan_id = '{plan_id}'; \
         DELETE FROM plan_metadata WHERE plan_id = '{plan_id}'; \
         DELETE FROM plans WHERE plan_id = '{plan_id}';"
    );
    let _ = bin().args(["db", "exec", &sql]).output();
}

/// Run a `travel` subcommand. Returns (ok, stdout, stderr).
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

/// COUNT(*) helper (db exec renders `SELECT COUNT(*) AS n` as `n: <count>`).
fn count(sql: &str) -> Option<i64> {
    let out = db_exec(sql)?;
    let n = out
        .lines()
        .find_map(|l| l.strip_prefix("n: "))
        .map(|s| s.trim().parse::<i64>().unwrap_or(-1))
        .unwrap_or(0);
    Some(n)
}

#[test]
fn full_maps_fresh_lifecycle() {
    let tag = nanos();
    let plan_id = format!("test-mapsfresh-{tag}");
    let dest = format!("mapsfresh_{tag}");

    if !seed(&plan_id, &dest) {
        return; // credless skip
    }

    // Always tear down, even on assertion failure.
    let result = std::panic::catch_unwind(|| {
        // 1. No snapshot row yet → check-maps-fresh warns "never snapshotted".
        let (ok, stdout, stderr) = run_cmd(&["check-maps-fresh", "--plan-id", &plan_id]);
        assert!(ok, "check-maps-fresh should exit 0 (advisory). stderr: {stderr}");
        assert!(
            stdout.contains("never snapshotted") && stdout.contains(&plan_id),
            "expected 'never snapshotted' warning for {plan_id}, got:\n{stdout}"
        );

        // 2. mark-maps-snapshotted upserts exactly one row.
        let (ok, stdout, stderr) = run_cmd(&["mark-maps-snapshotted", &plan_id]);
        assert!(ok, "mark-maps-snapshotted should succeed. stderr: {stderr}");
        assert!(
            stdout.contains("marked maps snapshotted") && stdout.contains(&plan_id),
            "unexpected mark output:\n{stdout}"
        );
        let n = count(&format!(
            "SELECT COUNT(*) AS n FROM plan_map_snapshots WHERE plan_id = '{plan_id}'"
        ))
        .expect("count after mark");
        assert_eq!(n, 1, "exactly one snapshot row expected after mark");

        // Upsert idempotency: a second mark stays at one row.
        let (ok, _, _) = run_cmd(&["mark-maps-snapshotted", &plan_id]);
        assert!(ok, "second mark-maps-snapshotted should succeed");
        let n = count(&format!(
            "SELECT COUNT(*) AS n FROM plan_map_snapshots WHERE plan_id = '{plan_id}'"
        ))
        .expect("count after 2nd mark");
        assert_eq!(n, 1, "upsert must not create a duplicate row");

        // 3. Right after snapshot, itinerary is older or equal → fresh.
        //    Force the seed rows to be OLDER than the snapshot to avoid a
        //    same-second tie reading as stale.
        db_exec(&format!(
            "UPDATE activities SET updated_at = datetime('now','-1 hour') WHERE plan_id = '{plan_id}'; \
             UPDATE days SET updated_at = datetime('now','-1 hour') WHERE plan_id = '{plan_id}';"
        ));
        let (ok, stdout, _) = run_cmd(&["check-maps-fresh", "--plan-id", &plan_id]);
        assert!(ok, "check-maps-fresh exit 0");
        assert!(
            stdout.contains("maps fresh") && stdout.contains(&plan_id),
            "expected 'maps fresh' after snapshot, got:\n{stdout}"
        );

        // 4. Touch an activity to be NEWER than the snapshot → STALE.
        db_exec(&format!(
            "UPDATE activities SET updated_at = datetime('now','+1 hour') WHERE plan_id = '{plan_id}'"
        ));
        let (ok, stdout, _) = run_cmd(&["check-maps-fresh", "--plan-id", &plan_id]);
        assert!(ok, "check-maps-fresh exit 0 even when stale (advisory)");
        assert!(
            stdout.contains("STALE") && stdout.contains(&plan_id),
            "expected STALE warning after itinerary edit, got:\n{stdout}"
        );
    });

    teardown(&plan_id);
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
