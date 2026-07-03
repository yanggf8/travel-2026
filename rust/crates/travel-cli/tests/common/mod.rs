//! Shared test helpers.
//!
//! `tests/common/mod.rs` is the Cargo idiom for code shared across integration test
//! binaries WITHOUT it becoming its own test binary. Each test file opts in with
//! `mod common;` and `use common::Guard;`.

use std::process::Command;

const PLAN_ID_TABLES_SQL: &str = "\
SELECT m.name
FROM sqlite_master m
WHERE m.type = 'table'
  AND EXISTS (
    SELECT 1 FROM pragma_table_info(m.name) p WHERE p.name = 'plan_id'
  )
ORDER BY CASE WHEN m.name = 'plans' THEN 1 ELSE 0 END, m.name";

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_travel")
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
