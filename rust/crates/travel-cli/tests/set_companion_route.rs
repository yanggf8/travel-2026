//! Real-Turso behavior lock for `set-companion-route`.
//!
//! Locks the audited write path (options + ordered stops, literal-coord and
//! NULL-coord stops), the audit triad, and the kimi-review fixes: unknown
//! `--dest` fails loud, clearing a nonexistent option fails instead of
//! bumping version. `db migrate` creates the companion tables first. The
//! Guard tears the plan down on panic.

use std::process::Command;

mod common;
use common::{Guard, bin, db_exec, is_credless, nanos, seed_plan, teardown_plan};

fn exec_ok(sql: &str) -> common::Rows {
    db_exec(sql).unwrap_or_else(|| panic!("db exec skipped unexpectedly for SQL: {sql}"))
}

fn run_cmd(args: &[&str]) -> Option<(bool, String, String)> {
    let out = Command::new(bin())
        .args(args)
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if is_credless(&stderr) {
        eprintln!("skipping set-companion-route lock: {}", stderr.trim());
        return None;
    }
    Some((out.status.success(), stdout, stderr))
}

fn scalar(sql: &str) -> String {
    exec_ok(sql).scalar().unwrap_or_default()
}

#[test]
fn set_companion_route_writes_stops_and_audits() {
    if db_exec("SELECT 1 AS n").is_none() {
        eprintln!("skipping set-companion-route lock (no Turso creds)");
        return;
    }
    let (ok, _, stderr) = match run_cmd(&["db", "migrate"]) {
        Some(v) => v,
        None => return,
    };
    assert!(ok, "db migrate failed: stderr={stderr}");
    assert_eq!(
        scalar("SELECT COUNT(*) AS n FROM sqlite_master WHERE name = 'companion_options'"),
        "1",
        "companion_options must exist after migrate"
    );

    let tag = nanos();
    let plan = format!("zztest-comproute-{tag}");
    let dest = format!("zzcomproute_{tag}");
    teardown_plan(&plan, &dest);
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_plan(&plan, &dest, 2);

    let version_before: i64 = scalar(&format!(
        "SELECT version FROM plans WHERE plan_id = '{plan}'"
    ))
    .parse()
    .unwrap();

    // Happy path: literal lat,lon resolves without network; a bare label with
    // no legend/cache hit stores NULL and renders chip-only.
    let (ok, stdout, stderr) = match run_cmd(&[
        "set-companion-route",
        "A",
        "--title",
        "測試方案",
        "--stops",
        "定位站, 25.1,121.8",
        "--plan-id",
        &plan,
    ]) {
        Some(v) => v,
        None => return,
    };
    assert!(ok, "set failed: {stdout} {stderr}");
    assert!(stdout.contains("2 stops, 1 geocoded"), "{stdout}");

    assert_eq!(
        scalar(&format!(
            "SELECT title FROM companion_options WHERE plan_id = '{}{}'",
            plan, ""
        )),
        "測試方案"
    );
    assert_eq!(
        scalar(&format!(
            "SELECT COUNT(*) FROM companion_option_stops WHERE plan_id = '{plan}' AND lat IS NOT NULL"
        )),
        "1"
    );
    assert_eq!(
        scalar(&format!(
            "SELECT COUNT(*) FROM companion_option_stops WHERE plan_id = '{plan}' AND lat IS NULL"
        )),
        "1"
    );
    // Audit triad: event + version bump.
    assert_eq!(
        scalar(&format!(
            "SELECT COUNT(*) FROM plan_events WHERE plan_id = '{plan}' AND event = 'companion_route_set'"
        )),
        "1"
    );
    assert_eq!(
        scalar(&format!("SELECT version FROM plans WHERE plan_id = '{plan}'")),
        (version_before + 1).to_string(),
        "one set-companion-route = exactly one version bump"
    );

    // kimi-fix: clearing a NONEXISTENT option fails, no audit noise.
    let (ok, stdout, stderr) = match run_cmd(&[
        "set-companion-route",
        "Z",
        "--clear",
        "--plan-id",
        &plan,
    ]) {
        Some(v) => v,
        None => return,
    };
    assert!(
        !ok,
        "clearing a nonexistent option must fail, got ok: {stdout} {stderr}"
    );

    // kimi-fix: unknown --dest fails loud instead of storing all-NULL stops.
    let (ok, stdout, stderr) = match run_cmd(&[
        "set-companion-route",
        "B",
        "--title",
        "x",
        "--stops",
        "某站",
        "--dest",
        &format!("zznosuch_{tag}"),
        "--plan-id",
        &plan,
    ]) {
        Some(v) => v,
        None => return,
    };
    assert!(
        !ok,
        "unknown --dest must fail loud, got ok: {stdout} {stderr}"
    );
    assert!(
        stderr.contains("unknown --dest"),
        "error must name the bad slug: {stderr}"
    );

    // Clearing the EXISTING option succeeds and removes the rows.
    let (ok, _, stderr) = match run_cmd(&["set-companion-route", "A", "--clear", "--plan-id", &plan])
    {
        Some(v) => v,
        None => return,
    };
    assert!(ok, "clear existing failed: {stderr}");
    assert_eq!(
        scalar(&format!(
            "SELECT COUNT(*) FROM companion_option_stops WHERE plan_id = '{plan}'"
        )),
        "0"
    );
}
