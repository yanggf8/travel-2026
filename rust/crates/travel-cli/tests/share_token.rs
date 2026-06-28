//! Integration test for the `share-token` command + `plan_share_tokens` table.
//!
//! `travel share-token <plan>` mints an opaque per-plan, view-scope token, stores
//! it in `plan_share_tokens`, and prints the token plus a share URL. The Worker
//! (read path) consumes that table; the CLI is the sole write path.
//!
//! Pattern mirrors set_mutation_bugs.rs: seed a throwaway plan, run the binary,
//! SELECT to assert, tear down. Skips cleanly when Turso creds are absent.

mod common;
use common::Guard;

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
        eprintln!("skipping share-token Turso test: {}", stderr.trim());
        return None;
    }
    panic!("travel db exec failed: {}\nSQL: {sql}", stderr.trim());
}

/// Seed only `plans` + `plan_metadata`. Returns false on a credless skip.
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
        "DELETE FROM plan_share_tokens WHERE plan_id = '{plan_id}'; \
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

#[test]
fn share_token_mints_and_persists_a_view_scope_token() {
    let tag = nanos();
    let plan_id = format!("test-sharetoken-{tag}");
    let dest = format!("sharetoken_{tag}");
    let _g = Guard::new({
        let plan_id = plan_id.clone();
        move || teardown(&plan_id)
    });
    if !seed_bare(&plan_id, &dest) {
        return;
    }

    // `share-token` resolves the plan via TRAVEL_PLAN_ID (set by run_cmd).
    let (ok, stdout, stderr) = run_cmd(&plan_id, &["share-token"]);

    let row_count = count(&format!(
        "SELECT COUNT(*) AS n FROM plan_share_tokens WHERE plan_id = '{plan_id}'"
    ));
    let token_row = db_exec(&format!(
        "SELECT token FROM plan_share_tokens WHERE plan_id = '{plan_id}'"
    ));

    assert!(ok, "share-token should succeed on a seeded plan; stdout={stdout} stderr={stderr}");
    assert_eq!(
        row_count,
        Some(1),
        "exactly one plan_share_tokens row must be persisted for the plan"
    );

    // The DB token must be non-empty.
    let db_token = token_row
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("token: ").map(|s| s.trim().to_string()))
        .unwrap_or_default();
    assert!(!db_token.is_empty(), "the persisted token must be non-empty");

    // The command's stdout must contain the minted token.
    assert!(
        stdout.contains(&db_token),
        "stdout must echo the minted token; stdout={stdout} db_token={db_token}"
    );
}
