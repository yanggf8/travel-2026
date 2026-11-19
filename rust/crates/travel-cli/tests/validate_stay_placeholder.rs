//! `validate data` stay-placeholder lint (spec 2026-11-19 §2 A3): once a plan's
//! P4 (process_4_accommodation) is booked/confirmed, an undecided-stay
//! placeholder (住宿待定/住宿未定) left in a route endpoint or activity title is
//! STALE — the stay is decided, the booked pin renders, the placeholder text
//! lies. While the stay is still undecided the placeholder is the convention
//! (routing.md 國內行程), so selecting/pending plans must stay quiet.
//!
//! Real-Turso integration test; scoped by unique zz fixtures; skips cleanly if
//! creds are absent. Serialized per file via a static mutex.

mod common;
use common::{bin, db_exec, nanos, seed_plan, teardown_plan, Guard};

use std::process::Command;
use std::sync::Mutex;

static SP_LOCK: Mutex<()> = Mutex::new(());

fn run_cmd(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin())
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

/// Seed the plan+destination's P4 process row at `status`.
fn seed_p4(plan: &str, dest: &str, status: &str) {
    db_exec(&format!(
        "INSERT OR REPLACE INTO process_statuses (plan_id, destination, process_id, status) \
         VALUES ('{plan}', '{dest}', 'process_4_accommodation', '{status}')"
    ))
    .expect("seed p4");
}

/// Seed one day_route_segments row (driving, like a self-drive domestic trip).
fn seed_segment(plan: &str, dest: &str, day: i64, sort: i64, from: &str, to: &str) {
    db_exec(&format!(
        "INSERT OR REPLACE INTO day_route_segments \
           (plan_id, destination, day_number, sort_order, from_place, to_place, mode, source) \
         VALUES ('{plan}', '{dest}', {day}, {sort}, '{from}', '{to}', 'driving', 'confirmed')"
    ))
    .expect("seed segment");
}

/// Seed one afternoon activity.
fn seed_activity(plan: &str, dest: &str, id: &str, day: i64, title: &str) {
    db_exec(&format!(
        "INSERT OR REPLACE INTO activities \
           (id, plan_id, destination, day_number, session_type, sort_order, title, \
            booking_required, is_fixed_time, priority, source) \
         VALUES ('{id}', '{plan}', '{dest}', {day}, 'afternoon', 0, '{title}', 0, 0, 'want', 'confirmed')"
    ))
    .expect("seed activity");
}

/// [stay-placeholder] lines of `validate data` naming this plan.
fn placeholder_hits(stdout: &str, plan: &str) -> Vec<String> {
    stdout
        .lines()
        .filter(|l| l.contains("[stay-placeholder]") && l.contains(plan))
        .map(str::to_string)
        .collect()
}

#[test]
fn stay_placeholder_lint_fires_only_for_decided_stays() {
    let _g = SP_LOCK.lock().unwrap();
    let tag = nanos();
    let plan = format!("zz_test_{tag}_sp");
    let dest = format!("zz_test_{tag}_sd");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || {
            teardown_plan(&plan, &dest);
        }
    });
    if db_exec("SELECT 1").is_none() {
        eprintln!("skipping (no Turso creds)");
        return;
    }
    seed_plan(&plan, &dest, 0);

    // Case 1 — P4 booked + placeholders in a segment endpoint and an activity
    // title → one WARN each, naming the source column and the fix commands.
    seed_p4(&plan, &dest, "booked");
    seed_segment(&plan, &dest, 1, 0, "紅樹林", "頭城（住宿待定）");
    seed_activity(&plan, &dest, &format!("zz_test_{tag}_act"), 1, "抵達頭城（住宿未定，也可能當天回）");
    let (ok, stdout, stderr) = run_cmd(&["validate", "data"]);
    assert!(ok, "placeholder lint is a warning, not a failure; stderr={stderr}");
    let hits = placeholder_hits(&stdout, &plan);
    assert_eq!(hits.len(), 2, "segment endpoint + activity title each WARN; got:\n{hits:#?}");
    assert!(
        hits.iter().any(|l| l.contains("day_route_segments.to_place") && l.contains("住宿待定")),
        "segment hit names the endpoint: {hits:#?}"
    );
    assert!(
        hits.iter().any(|l| l.contains("activities.title") && l.contains("住宿未定")),
        "activity hit names the title: {hits:#?}"
    );
    assert!(
        hits.iter().all(|l| l.contains("update-activity") && l.contains("set-route-segment")),
        "fix commands named: {hits:#?}"
    );

    // Case 2 — same plan flipped back to selecting: the placeholder IS the
    // convention while the stay is undecided → no WARN.
    db_exec(&format!(
        "UPDATE process_statuses SET status='selecting' \
         WHERE plan_id='{plan}' AND destination='{dest}' AND process_id='process_4_accommodation'"
    ))
    .expect("flip p4 to selecting");
    let (ok, stdout, stderr) = run_cmd(&["validate", "data"]);
    assert!(ok, "stderr={stderr}");
    assert!(
        placeholder_hits(&stdout, &plan).is_empty(),
        "selecting + placeholder is the convention, not a defect: {}",
        stdout.lines().filter(|l| l.contains("stay-placeholder")).collect::<Vec<_>>().join("\n")
    );

    // Case 3 — booked again but the placeholders are replaced by the real stay
    // → no WARN (the lint keys on the placeholder text, not on booked P4 alone).
    db_exec(&format!(
        "UPDATE process_statuses SET status='booked' \
         WHERE plan_id='{plan}' AND destination='{dest}' AND process_id='process_4_accommodation'"
    ))
    .expect("flip p4 back to booked");
    db_exec(&format!(
        "UPDATE day_route_segments SET to_place='頭城晏京' WHERE plan_id='{plan}' AND destination='{dest}'"
    ))
    .expect("replace placeholder endpoint");
    db_exec(&format!(
        "UPDATE activities SET title='抵達頭城入住' WHERE plan_id='{plan}' AND destination='{dest}'"
    ))
    .expect("replace placeholder title");
    let (ok, stdout, stderr) = run_cmd(&["validate", "data"]);
    assert!(ok, "stderr={stderr}");
    assert!(
        placeholder_hits(&stdout, &plan).is_empty(),
        "booked with real stay text stays quiet: {}",
        stdout.lines().filter(|l| l.contains("stay-placeholder")).collect::<Vec<_>>().join("\n")
    );
}
