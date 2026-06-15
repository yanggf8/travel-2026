//! Regression tests for the `db seed <ref-data>` commands (ported from the
//! retired `scripts/seed-*.ts`). These seeders build INSERT statements as SQL
//! string literals; a copy-paste once mangled the `fetched_at` column into
//! `fetched_at.clone()` inside the SQL text, so every seeder crashed at runtime
//! with `sqlite3 parser error: near DOT`. The crash was invisible because the
//! seeders had no test coverage. These tests run each seeder against real Turso
//! and assert rows land, so a malformed INSERT fails the suite instead of only
//! the next human who runs `db seed`.
//!
//! Pattern mirrors set_mutation_bugs.rs: run the binary, SELECT, assert. Skips
//! cleanly when Turso creds are absent. The seeders use `INSERT OR REPLACE`
//! (idempotent) and target shared global reference tables, so there is no
//! per-test plan to seed or tear down.

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_travel"))
}

fn is_credless(stderr: &str) -> bool {
    stderr.contains("turso auth login")
        || stderr.contains("Missing Turso data")
        || stderr.contains("failed to connect to Turso")
        || stderr.contains("TRAVEL_TURSO")
}

/// Run a `travel` subcommand. Returns (ok, stdout, stderr).
fn run(args: &[&str]) -> (bool, String, String) {
    let out = bin()
        .args(args)
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
    let out = bin().args(["db", "exec", sql]).output().expect("db exec");
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if is_credless(&stderr) {
            eprintln!("skipping db-seed Turso test: {}", stderr.trim());
            return None;
        }
        panic!("travel db exec failed: {}\nSQL: {sql}", stderr.trim());
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let n = stdout
        .lines()
        .find_map(|l| l.strip_prefix("n: "))
        .map(|s| s.trim().parse::<i64>().unwrap_or(-1))
        .unwrap_or(0);
    Some(n)
}

// ── db seed ota-knowledge populates airlines (and friends) ───────────────────
// The airlines INSERT is the one whose mangled SQL string crashed first.
#[test]
fn db_seed_ota_knowledge_populates_airlines() {
    // Skip cleanly if creds are absent.
    if count("SELECT COUNT(*) AS n FROM airlines").is_none() {
        return;
    }

    let (ok, stdout, stderr) = run(&["db", "seed", "ota-knowledge"]);
    assert!(
        ok,
        "db seed ota-knowledge must succeed (no malformed INSERT); stdout={stdout} stderr={stderr}"
    );

    assert!(
        count("SELECT COUNT(*) AS n FROM airlines").unwrap_or(0) > 0,
        "airlines must be populated after `db seed ota-knowledge`"
    );
    assert!(
        count("SELECT COUNT(*) AS n FROM comparison_rules").unwrap_or(0) > 0,
        "comparison_rules must be populated after `db seed ota-knowledge`"
    );
}

// ── db seed destination-refs populates destination_areas (and friends) ───────
#[test]
fn db_seed_destination_refs_populates_areas() {
    if count("SELECT COUNT(*) AS n FROM destination_areas").is_none() {
        return;
    }

    let (ok, stdout, stderr) = run(&["db", "seed", "destination-refs"]);
    assert!(
        ok,
        "db seed destination-refs must succeed (no malformed INSERT); stdout={stdout} stderr={stderr}"
    );

    assert!(
        count("SELECT COUNT(*) AS n FROM destination_areas").unwrap_or(0) > 0,
        "destination_areas must be populated after `db seed destination-refs`"
    );
    assert!(
        count("SELECT COUNT(*) AS n FROM destination_pois").unwrap_or(0) > 0,
        "destination_pois must be populated after `db seed destination-refs`"
    );
}
