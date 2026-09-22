//! Real-Turso behavior lock for `set-fit-note`.
//!
//! Writes the comparison paragraph and one agency reason, switches the single
//! Recommended pick, clears a note, and checks the audit triad. `db migrate`
//! creates `plan_fit_notes` first. The Guard tears the plan down on panic.

use std::process::Command;

mod common;
use common::{Guard, bin, db_exec, is_credless, nanos, seed_plan, teardown_plan};

fn sql_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

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
        eprintln!("skipping set-fit-note lock: {}", stderr.trim());
        return None;
    }
    Some((out.status.success(), stdout, stderr))
}

fn scalar(sql: &str) -> String {
    exec_ok(sql).scalar().unwrap_or_default()
}

#[test]
fn set_fit_note_writes_one_pick_and_audits_it() {
    if db_exec("SELECT 1 AS n").is_none() {
        eprintln!("skipping set-fit-note lock (no Turso creds)");
        return;
    }
    let (ok, _, stderr) = match run_cmd(&["db", "migrate"]) {
        Some(v) => v,
        None => return,
    };
    assert!(ok, "db migrate failed: stdout discarded; stderr={stderr}");
    assert_eq!(
        scalar("SELECT COUNT(*) AS n FROM sqlite_master WHERE name = 'plan_fit_notes'"),
        "1",
        "plan_fit_notes must exist after migrate"
    );
    assert_eq!(
        scalar(
            "SELECT COUNT(*) AS n FROM sqlite_master WHERE name = 'idx_plan_fit_notes_one_pick'"
        ),
        "1",
        "one-pick unique index must exist after migrate"
    );

    let tag = nanos();
    let plan = format!("zztest-fitnote-{tag}");
    let dest = format!("zzfitnote_{tag}");
    teardown_plan(&plan, &dest);
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_plan(&plan, &dest, 10);

    let compare = match run_cmd(&["set-fit-note", "--zh", "三家比較", "--plan-id", &plan]) {
        Some(v) => v,
        None => return,
    };
    assert!(
        compare.0,
        "compare note failed: {} {}",
        compare.1, compare.2
    );
    assert!(compare.1.contains("source: (comparison)"), "{}", compare.1);

    let pick = match run_cmd(&[
        "set-fit-note",
        "--source",
        "lifetour",
        "--zh",
        "五福最低",
        "--en",
        "LifeTour lowest",
        "--recommend",
        "--plan-id",
        &plan,
    ]) {
        Some(v) => v,
        None => return,
    };
    assert!(pick.0, "recommend failed: {} {}", pick.1, pick.2);
    assert!(pick.1.contains("recommended: yes"), "{}", pick.1);

    let p = sql_lit(&plan);
    let d = sql_lit(&dest);
    assert_eq!(
        scalar(&format!(
            "SELECT body_zh AS v FROM plan_fit_notes WHERE plan_id = {p} AND destination = {d} AND source_id = ''"
        )),
        "三家比較"
    );
    assert_eq!(
        scalar(&format!(
            "SELECT recommended AS v FROM plan_fit_notes WHERE plan_id = {p} AND destination = {d} AND source_id = 'lifetour'"
        )),
        "1"
    );

    // Switching the pick clears the previous agency. Only one recommended row.
    let switched = match run_cmd(&[
        "set-fit-note",
        "--source",
        "liontravel",
        "--zh",
        "改推雄獅",
        "--recommend",
        "--plan-id",
        &plan,
    ]) {
        Some(v) => v,
        None => return,
    };
    assert!(switched.0, "switch failed: {} {}", switched.1, switched.2);
    assert_eq!(
        scalar(&format!(
            "SELECT recommended AS v FROM plan_fit_notes WHERE plan_id = {p} AND destination = {d} AND source_id = 'lifetour'"
        )),
        "0"
    );
    assert_eq!(
        scalar(&format!(
            "SELECT COUNT(*) AS n FROM plan_fit_notes WHERE plan_id = {p} AND destination = {d} AND recommended = 1"
        )),
        "1"
    );

    assert_eq!(
        scalar(&format!(
            "SELECT version AS v FROM plans WHERE plan_id = {p}"
        )),
        "13",
        "seed 10 + compare + lifetour + liontravel"
    );
    assert_eq!(
        scalar(&format!(
            "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id = {p} AND command_type = 'set-fit-note'"
        )),
        "3"
    );
    assert_eq!(
        scalar(&format!(
            "SELECT COUNT(*) AS n FROM plan_events WHERE plan_id = {p} AND event = 'fit_note_set'"
        )),
        "3"
    );

    let refused = match run_cmd(&[
        "set-fit-note",
        "--zh",
        "段落",
        "--recommend",
        "--plan-id",
        &plan,
    ]) {
        Some(v) => v,
        None => return,
    };
    assert!(
        !refused.0,
        "recommend on the paragraph must fail: {}",
        refused.1
    );
    assert!(refused.2.contains("--recommend") || refused.1.contains("--recommend"));

    let cleared = match run_cmd(&[
        "set-fit-note",
        "--source",
        "lifetour",
        "--clear",
        "--plan-id",
        &plan,
    ]) {
        Some(v) => v,
        None => return,
    };
    assert!(cleared.0, "clear failed: {} {}", cleared.1, cleared.2);
    assert_eq!(
        scalar(&format!(
            "SELECT COUNT(*) AS n FROM plan_fit_notes WHERE plan_id = {p} AND destination = {d} AND source_id = 'lifetour'"
        )),
        "0"
    );
    assert_eq!(
        scalar(&format!(
            "SELECT COUNT(*) AS n FROM plan_events WHERE plan_id = {p} AND event = 'fit_note_cleared'"
        )),
        "1"
    );

    let missing = match run_cmd(&[
        "set-fit-note",
        "--source",
        "lifetour",
        "--clear",
        "--plan-id",
        &plan,
    ]) {
        Some(v) => v,
        None => return,
    };
    assert!(!missing.0, "clearing a missing note must fail");
}
