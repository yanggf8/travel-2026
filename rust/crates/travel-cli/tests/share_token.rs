//! Integration test for the `share-token` command + `plan_share_tokens` table.
//!
//! `travel share-token <plan>` mints an opaque per-plan, view-scope token, stores
//! it in `plan_share_tokens`, and prints the token plus a share URL. The Worker
//! (read path) consumes that table; the CLI is the sole write path.
//!
//! Pattern mirrors set_mutation_bugs.rs: seed a throwaway plan, run the binary,
//! SELECT to assert, tear down. Skips cleanly when Turso creds are absent.

mod common;
use common::{bin, db_exec, nanos, seed_plan, teardown_plan, Guard};

use std::process::Command;

/// Run a `travel` subcommand with TRAVEL_PLAN_ID set. Returns (ok, stdout, stderr).
fn run_cmd(plan_id: &str, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin())
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
    let n = db_exec(sql)?
        .scalar()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    Some(n)
}

#[test]
fn share_token_mints_and_persists_a_view_scope_token() {
    let tag = nanos();
    let plan_id = format!("test-sharetoken-{tag}");
    let dest = format!("sharetoken_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown_plan(&plan_id, &dest)
    });
    if db_exec("SELECT 1").is_none() {
        eprintln!("skipping share-token Turso test (no Turso creds)");
        return;
    }
    seed_plan(&plan_id, &dest, 0);

    // `share-token` resolves the plan via TRAVEL_PLAN_ID (set by run_cmd).
    let (ok, stdout, stderr) = run_cmd(&plan_id, &["share-token"]);

    let row_count = count(&format!(
        "SELECT COUNT(*) AS n FROM plan_share_tokens WHERE plan_id = '{plan_id}'"
    ));
    let token_row = db_exec(&format!(
        "SELECT token FROM plan_share_tokens WHERE plan_id = '{plan_id}'"
    ))
    .map(|rows| rows.raw().to_string());

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