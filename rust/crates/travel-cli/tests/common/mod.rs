//! Shared test helpers.
//!
//! `tests/common/mod.rs` is the Cargo idiom for code shared across integration test
//! binaries WITHOUT it becoming its own test binary. Each test file opts in with
//! `mod common;` and `use common::Guard;`.
//!
//! `allow(dead_code)`: this module is compiled fresh into EACH integration-test binary,
//! and each binary uses only the subset of helpers its tests need — so every helper is
//! "unused" from some binary's view. That's the standard `tests/common` idiom, not a bug.
#![allow(dead_code)]

use std::process::Command;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const PLAN_ID_TABLES_SQL: &str = "\
SELECT m.name
FROM sqlite_master m
WHERE m.type = 'table'
  AND EXISTS (
    SELECT 1 FROM pragma_table_info(m.name) p WHERE p.name = 'plan_id'
  )
ORDER BY CASE WHEN m.name = 'plans' THEN 1 ELSE 0 END, m.name";

pub fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_travel")
}

pub fn nanos() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
}

pub fn is_credless(stderr: &str) -> bool {
    stderr.contains("turso auth login")
        || stderr.contains("Missing Turso")
        || stderr.contains("Missing Turso data")
        || stderr.contains("failed to connect to Turso")
        || stderr.contains("TRAVEL_TURSO")
        || stderr.contains("database URL")
}

pub fn is_transient(stderr: &str) -> bool {
    stderr.contains("Network is unreachable")
        || stderr.contains("error trying to connect")
        || stderr.contains("connection reset")
        || stderr.contains("connection closed")
}

#[derive(Clone, Debug)]
pub struct Rows {
    pub stdout: String,
    pub stderr: String,
}

impl Rows {
    pub fn scalar(&self) -> Option<String> {
        self.stdout
            .lines()
            .find_map(|line| strip_batch_prefix(line).split_once(':').map(|(_, v)| v.trim().to_string()))
    }

    pub fn column(&self) -> Vec<String> {
        self.stdout
            .lines()
            .filter_map(|line| strip_batch_prefix(line).split_once(':').map(|(_, v)| v.trim().to_string()))
            .filter(|v| !v.is_empty())
            .collect()
    }

    pub fn raw(&self) -> &str {
        &self.stdout
    }
}

impl std::fmt::Display for Rows {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.stdout)
    }
}

fn strip_batch_prefix(line: &str) -> &str {
    let line = line.trim_start();
    let Some(rest) = line.strip_prefix('[') else {
        return line;
    };
    let Some((prefix, after)) = rest.split_once("] ") else {
        return line;
    };
    let Some((left, right)) = prefix.split_once('/') else {
        return line;
    };

    if !left.is_empty()
        && !right.is_empty()
        && left.chars().all(|c| c.is_ascii_digit())
        && right.chars().all(|c| c.is_ascii_digit())
    {
        after
    } else {
        line
    }
}

pub fn db_exec(sql: &str) -> Option<Rows> {
    for attempt in 0..6 {
        let out = Command::new(bin())
            .args(["db", "exec", sql])
            .env_remove("TRAVEL_PLAN_ID")
            .output()
            .expect("run db exec");

        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();

        if !out.status.success() && is_credless(&stderr) {
            return None;
        }

        if !out.status.success() && is_transient(&stderr) && attempt < 5 {
            thread::sleep(Duration::from_millis(400 * (attempt as u64 + 1)));
            continue;
        }

        if !out.status.success() {
            panic!("travel db exec failed: {}\nSQL: {sql}", stderr.trim());
        }

        return Some(Rows { stdout, stderr });
    }

    unreachable!()
}

pub fn seed_plan(plan: &str, dest: &str, version: i64) {
    let plan = sql_lit(plan);
    let dest = sql_lit(dest);
    // A production plan ALWAYS has its active destination registered in
    // plan_destinations (verified: every live plan does). Seed it too, so tests
    // that pass `--dest <the active dest>` match reality — `resolve_active_destination`
    // validates the override against plan_destinations (fail-loud on a phantom dest).
    // teardown_plan's dynamic sqlite_master scan already covers plan_destinations.
    db_exec(&format!(
        "INSERT OR REPLACE INTO plans (plan_id, schema_version, version) \
         VALUES ({plan}, '4.2.0', {version}); \
         INSERT OR REPLACE INTO plan_metadata (plan_id, schema_version, active_destination) \
         VALUES ({plan}, '4.2.0', {dest}); \
         INSERT OR REPLACE INTO plan_destinations (plan_id, slug, display_name, status) \
         VALUES ({plan}, {dest}, 'Seed Dest', 'active');"
    ))
    .expect("seed plan");
}

fn sql_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn sql_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// Best-effort DB exec for teardown paths only.
///
/// This must never panic: no Turso creds, missing auth, network failure, or a stale
/// table reference should leave the test panic path able to unwind cleanly.
pub fn db_exec_teardown(sql: &str) -> Option<String> {
    let out = Command::new(bin())
        .args(["db", "exec", sql])
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .ok()?;

    if !out.status.success() {
        return None;
    }

    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Delete every live table that has a `plan_id` column for this plan.
///
/// `dest` is intentionally unused: old test teardowns drifted because each test
/// guessed the destination-scoped write surface. Test plan IDs are unique, so the
/// canonical scope is the whole plan.
pub fn teardown_plan(plan: &str, dest: &str) {
    let _ = dest;

    let Some(stdout) = db_exec_teardown(PLAN_ID_TABLES_SQL) else {
        return;
    };

    let tables: Vec<String> = stdout
        .lines()
        .filter_map(|line| line.split_once(':').map(|(_, v)| v.trim().to_string()))
        .filter(|name| !name.is_empty())
        .collect();

    if tables.is_empty() {
        return;
    }

    let plan = sql_lit(plan);
    let mut sql = String::new();

    for table in tables {
        sql.push_str("DELETE FROM ");
        sql.push_str(&sql_ident(&table));
        sql.push_str(" WHERE plan_id = ");
        sql.push_str(&plan);
        sql.push_str("; ");
    }

    let _ = db_exec_teardown(&sql);
}

/// Delete global `offers` rows owned by a test. These rows are not plan-keyed.
pub fn teardown_offers(ids: &[&str]) {
    if ids.is_empty() {
        return;
    }

    let mut sql = String::new();
    for id in ids {
        sql.push_str("DELETE FROM offers WHERE id = ");
        sql.push_str(&sql_lit(id));
        sql.push_str("; ");
    }

    let _ = db_exec_teardown(&sql);
}

/// RAII teardown guard: runs the wrapped closure on `Drop` — i.e. on BOTH normal
/// return AND panic-unwind.
///
/// Integration tests historically called `teardown(...)` as the LAST statement of the
/// test. A panicking assertion (every TDD RED run, and any real regression) unwinds the
/// stack past that trailing call, so teardown never runs and test rows LEAK into the
/// shared Turso DB. Wrapping teardown in a `Guard` closes that hole: the closure fires
/// during unwinding.
///
/// Usage:
/// ```ignore
/// mod common;
/// use common::Guard;
/// // ... build plan/dest ...
/// teardown(&plan, &dest);                 // optional defensive pre-clean
/// let _g = Guard::new({
///     let (plan, dest) = (plan.clone(), dest.clone());
///     move || teardown(&plan, &dest)
/// });
/// // ... seed, run, assert (any panic still tears down) ...
/// ```
pub struct Guard<F: FnMut()>(F);

impl<F: FnMut()> Guard<F> {
    pub fn new(f: F) -> Self {
        Guard(f)
    }
}

impl<F: FnMut()> Drop for Guard<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}
