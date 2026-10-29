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
/// Status of one seeded token, or None on a credless skip.
fn token_status(token: &str) -> Option<String> {
    db_exec(&format!(
        "SELECT status FROM plan_share_tokens WHERE token = '{token}'"
    ))?
    .scalar()
}

#[test]
fn share_token_multi_deactivate_and_all_listing() {
    let tag = nanos();
    let plan_id = format!("test-sharemulti-{tag}");
    let dest = format!("sharemulti_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown_plan(&plan_id, &dest)
    });
    if db_exec("SELECT 1").is_none() {
        eprintln!("skipping share-token multi-deactivate Turso test (no Turso creds)");
        return;
    }
    seed_plan(&plan_id, &dest, 0);

    // Per-run unique 32-hex tokens (token is the GLOBAL primary key) with
    // distinct 6-char heads, so a fingerprint-head prefix resolves uniquely.
    let head = (tag % 0xff_fff0) as u32;
    let toks: Vec<String> = (0..3u32)
        .map(|i| format!("{:06x}{:026x}", head + i, tag))
        .collect();
    for t in &toks {
        db_exec(&format!(
            "INSERT INTO plan_share_tokens (plan_id, token, created_at, status) \
             VALUES ('{plan_id}', '{t}', datetime('now'), 'active')"
        ));
    }

    // One subcommand list + a repeated flag: full token A, then B by prefix.
    // Both must flip — a repeated flag used to REPLACE the earlier list.
    let b_prefix = &toks[1][..8];
    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &["share-token", "deactivate", &toks[0], "--deactivate", b_prefix],
    );
    assert!(ok, "multi deactivate should succeed; stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("2 deactivated, 0 skipped"), "stdout={stdout}");
    assert_eq!(token_status(&toks[0]).as_deref(), Some("inactive"));
    assert_eq!(token_status(&toks[1]).as_deref(), Some("inactive"));
    assert_eq!(token_status(&toks[2]).as_deref(), Some("active"));

    // `--all` alone is rejected — it must never fall through to mint.
    let (ok, stdout, _) = run_cmd(&plan_id, &["share-token", "--all"]);
    assert!(!ok, "--all alone must be rejected; stdout={stdout}");
    // `--show --all` lists actives plus the unfolded deactivated block.
    let (ok, stdout, stderr) = run_cmd(&plan_id, &["share-token", "--show", "--all"]);
    assert!(ok, "--show --all should list; stdout={stdout} stderr={stderr}");
    assert_eq!(
        count(&format!(
            "SELECT COUNT(*) AS n FROM plan_share_tokens WHERE plan_id = '{plan_id}'"
        )),
        Some(3),
        "neither --all nor --show --all may mint a fresh token"
    );
    assert!(stdout.contains("停用 (2):"), "deactivated block unfolded; stdout={stdout}");
    assert!(stdout.contains(&toks[0][..6]), "stdout={stdout}");

    // The same token named twice (full + its own prefix) is ONE request:
    // success, reported as a duplicate, never a partial failure.
    let c_prefix = &toks[2][..8];
    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &["share-token", "deactivate", &toks[2], c_prefix],
    );
    assert!(ok, "duplicate naming must not fail; stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("1 deactivated, 0 skipped, 1 duplicate"), "stdout={stdout}");
    assert_eq!(token_status(&toks[2]).as_deref(), Some("inactive"));

    // Re-deactivating an already-inactive token fails loud, changes nothing.
    let (ok, stdout, stderr) =
        run_cmd(&plan_id, &["share-token", "deactivate", &toks[0]]);
    assert!(!ok, "inactive token must not report success; stdout={stdout}");
    assert!(stderr.contains("no active token"), "stderr={stderr}");
}
