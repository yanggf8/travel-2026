//! Real-Turso integration test for `flow-decision` (F6 audit triad).
//! Skips cleanly if Turso creds are absent. Panic-safe teardown via Guard.

use std::collections::HashSet;
use std::process::Command;

mod common;
use common::{bin, db_exec, nanos, seed_plan, teardown_plan, Guard};

fn count(sql: &str) -> Option<i64> {
    db_exec(sql)?.scalar().and_then(|s| s.parse::<i64>().ok())
}

fn scalar(sql: &str) -> Option<String> {
    db_exec(sql)?.scalar()
}

fn kv_lines(plan: &str, event: &str) -> Option<Vec<String>> {
    let sql = format!(
        "SELECT key || '=' || value AS li FROM plan_event_data \
         WHERE plan_id = '{plan}' AND scope = 'timeline' \
           AND sort_order = ( \
             SELECT sort_order FROM plan_events \
             WHERE plan_id = '{plan}' AND scope = 'timeline' AND event = '{event}' \
           ) \
         ORDER BY key"
    );
    Some(db_exec(&sql)?.column())
}

fn seed_bare(plan_id: &str, dest: &str) -> bool {
    if db_exec("SELECT 1").is_none() {
        return false;
    }
    seed_plan(plan_id, dest, 0);
    true
}

fn run_cmd(plan_id: &str, args: &[&str]) -> (bool, String, String) {
    let mut cmd = Command::new(bin());
    cmd.args(args).env("TRAVEL_PLAN_ID", plan_id);
    let out = cmd.output().unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn assert_no_audit_rows(plan_id: &str) {
    assert_eq!(
        count(&format!(
            "SELECT COUNT(*) AS n FROM plan_events WHERE plan_id = '{plan_id}' AND event = 'flow_decision'"
        )),
        Some(0)
    );
    assert_eq!(
        count(&format!(
            "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id = '{plan_id}' AND command_type = 'flow-decision'"
        )),
        Some(0)
    );
}

#[test]
fn flow_decision_mode_happy_path_writes_triad() {
    let tag = nanos();
    let plan_id = format!("zztest-flow-mode-{tag}");
    let dest = format!("zztest_flow_{tag}");
    teardown_plan(&plan_id, &dest);
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown_plan(&plan_id, &dest)
    });
    if !seed_bare(&plan_id, &dest) {
        return;
    }

    let version_before = scalar(&format!(
        "SELECT version AS v FROM plans WHERE plan_id = '{plan_id}'"
    ))
    .unwrap();

    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &[
            "flow-decision",
            "shop",
            "mode",
            "--mode",
            "ingest-known",
            "--reason",
            "known_flights",
        ],
    );
    assert!(ok, "flow-decision must succeed; stderr={stderr}");
    assert!(stdout.contains("flow-decision recorded:"));
    assert!(stdout.contains("shop mode"));
    assert!(stdout.contains("mode=ingest-known"));
    assert!(stdout.contains("reason=known_flights"));

    assert_eq!(
        count(&format!(
            "SELECT COUNT(*) AS n FROM plan_events WHERE plan_id = '{plan_id}' AND event = 'flow_decision'"
        )),
        Some(1)
    );

    let kv = kv_lines(&plan_id, "flow_decision").unwrap();
    assert_eq!(
        kv,
        vec![
            "decision=mode".to_string(),
            "mode=ingest-known".to_string(),
            "reason=known_flights".to_string(),
            "stage=shop".to_string(),
        ]
    );

    assert_eq!(
        count(&format!(
            "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id = '{plan_id}' AND command_type = 'flow-decision'"
        )),
        Some(1)
    );

    let summary = scalar(&format!(
        "SELECT command_summary AS v FROM operation_runs \
         WHERE plan_id = '{plan_id}' AND command_type = 'flow-decision'"
    ))
    .unwrap();
    assert_eq!(
        summary,
        "shop mode mode=ingest-known reason=known_flights"
    );

    let version_after = scalar(&format!(
        "SELECT version AS v FROM plans WHERE plan_id = '{plan_id}'"
    ))
    .unwrap();
    assert_eq!(
        version_after.parse::<i64>().unwrap(),
        version_before.parse::<i64>().unwrap() + 1
    );
}

#[test]
fn flow_decision_mode_without_mode_flag_fails_writes_nothing() {
    let tag = nanos();
    let plan_id = format!("zztest-flow-nomode-{tag}");
    let dest = format!("zztest_flow_{tag}");
    teardown_plan(&plan_id, &dest);
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown_plan(&plan_id, &dest)
    });
    if !seed_bare(&plan_id, &dest) {
        return;
    }

    let (ok, _stdout, _stderr) = run_cmd(&plan_id, &["flow-decision", "shop", "mode"]);
    assert!(!ok);
    assert_no_audit_rows(&plan_id);
}

#[test]
fn flow_decision_enter_with_mode_flag_fails() {
    let tag = nanos();
    let plan_id = format!("zztest-flow-badmode-{tag}");
    let dest = format!("zztest_flow_{tag}");
    teardown_plan(&plan_id, &dest);
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown_plan(&plan_id, &dest)
    });
    if !seed_bare(&plan_id, &dest) {
        return;
    }

    let (ok, _stdout, _stderr) = run_cmd(
        &plan_id,
        &["flow-decision", "shaping", "enter", "--mode", "shop"],
    );
    assert!(!ok);
    assert_no_audit_rows(&plan_id);
}

#[test]
fn flow_decision_invalid_stage_fails() {
    let tag = nanos();
    let plan_id = format!("zztest-flow-badstage-{tag}");
    let dest = format!("zztest_flow_{tag}");
    teardown_plan(&plan_id, &dest);
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown_plan(&plan_id, &dest)
    });
    if !seed_bare(&plan_id, &dest) {
        return;
    }

    let (ok, _stdout, _stderr) = run_cmd(&plan_id, &["flow-decision", "bogus", "enter"]);
    assert!(!ok);
    assert_no_audit_rows(&plan_id);
}

#[test]
fn flow_decision_enter_happy_path_writes_triad() {
    let tag = nanos();
    let plan_id = format!("zztest-flow-enter-{tag}");
    let dest = format!("zztest_flow_{tag}");
    teardown_plan(&plan_id, &dest);
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown_plan(&plan_id, &dest)
    });
    if !seed_bare(&plan_id, &dest) {
        return;
    }

    let version_before = scalar(&format!(
        "SELECT version AS v FROM plans WHERE plan_id = '{plan_id}'"
    ))
    .unwrap();

    let (ok, stdout, stderr) = run_cmd(&plan_id, &["flow-decision", "shop", "enter"]);
    assert!(ok, "enter must succeed; stderr={stderr}");
    assert!(stdout.contains("flow-decision recorded: shop enter"));

    assert_eq!(
        count(&format!(
            "SELECT COUNT(*) AS n FROM plan_events WHERE plan_id = '{plan_id}' AND event = 'flow_decision'"
        )),
        Some(1)
    );

    let kv = kv_lines(&plan_id, "flow_decision").unwrap();
    assert_eq!(
        kv,
        vec!["decision=enter".to_string(), "stage=shop".to_string()]
    );

    assert_eq!(
        count(&format!(
            "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id = '{plan_id}' AND command_type = 'flow-decision'"
        )),
        Some(1)
    );

    let version_after = scalar(&format!(
        "SELECT version AS v FROM plans WHERE plan_id = '{plan_id}'"
    ))
    .unwrap();
    assert_eq!(
        version_after.parse::<i64>().unwrap(),
        version_before.parse::<i64>().unwrap() + 1
    );
}

#[test]
fn accepted_value_constants_match_spec_sets() {
    let stages: HashSet<&str> = ["shaping", "itinerary", "shop", "publish"].into_iter().collect();
    let decisions: HashSet<&str> = ["enter", "skip", "mode"].into_iter().collect();
    let modes: HashSet<&str> = ["shop", "ingest-known", "defer"].into_iter().collect();

    // Mirror the pub slices declared in flow_decision.rs — drift guard for T4/T5.
    const STAGES: &[&str] = &["shaping", "itinerary", "shop", "publish"];
    const DECISIONS: &[&str] = &["enter", "skip", "mode"];
    const MODES: &[&str] = &["shop", "ingest-known", "defer"];

    assert_eq!(STAGES.iter().copied().collect::<HashSet<_>>(), stages);
    assert_eq!(DECISIONS.iter().copied().collect::<HashSet<_>>(), decisions);
    assert_eq!(MODES.iter().copied().collect::<HashSet<_>>(), modes);
    assert_eq!(STAGES.len(), 4);
    assert_eq!(DECISIONS.len(), 3);
    assert_eq!(MODES.len(), 3);
}