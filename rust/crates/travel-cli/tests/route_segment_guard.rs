//! Integration tests for the fail-loud guard bundle on the route-segment
//! writers (`set-route-segment` + `set-route-segments-bulk`).
//!
//! This is the bug class that broke the dashboard 3x: a junk stop (mode-only
//! word / empty-after-clean / clock-time), a cross-country ground leg
//! (TW↔Okinawa), or a walking leg that actually rides rail — each must be
//! REJECTED at WRITE time, before anything reaches the DB / dashboard. The
//! guard reuses the validate-itinerary lint's OWN predicates verbatim, so the
//! writer and the lint agree exactly.
//!
//! Invariants proven here:
//!   - a junk / cross-country / walking-over-rail segment exits non-zero and
//!     writes NOTHING (no day_route_segments row, no completed audit);
//!   - a clean, same-country, correctly-moded segment PASSES and persists;
//!   - a bulk batch with even one bad segment is rejected WHOLE — no
//!     half-write — and reports which segment(s) failed.
//!
//! Pattern mirrors set_mutation_bugs.rs: seed a throwaway plan + a single days
//! row, run the binary, SELECT to assert, tear down. Skips cleanly when Turso
//! creds are absent. NEVER touches a real plan.

mod common;
use common::{bin, db_exec, nanos, seed_plan, teardown_plan, Guard};

use std::process::Command;

/// Seed a throwaway plan + plan_metadata + ONE days row (day 1) so a guarded
/// write that PASSES isn't a no-op. Returns false on a credless skip.
fn seed_plan_with_day(plan_id: &str, dest: &str) -> bool {
    if db_exec("SELECT 1").is_none() {
        return false;
    }
    seed_plan(plan_id, dest, 0);
    let sql = format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type) \
           VALUES ('{plan_id}', '{dest}', 1, '2026-06-12', 'full');"
    );
    db_exec(&sql).is_some()
}

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

fn seg_count(plan_id: &str) -> Option<i64> {
    count(&format!(
        "SELECT COUNT(*) AS n FROM day_route_segments WHERE plan_id = '{plan_id}'"
    ))
}

fn completed_audit_count(plan_id: &str, cmd: &str) -> Option<i64> {
    count(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs \
         WHERE plan_id = '{plan_id}' AND command_type = '{cmd}' AND status = 'completed'"
    ))
}

// Reference Okinawa stops (UTF-8 escapes so the file stays ASCII):
const AKAMINE_EKI: &str = "\u{8D64}\u{5DBA}\u{99C5}"; // 赤嶺駅
const IIAS: &str = "iias \u{6C96}\u{7E04}\u{8C4A}\u{5D0E}"; // iias 沖縄豊崎
const WALK: &str = "\u{6B65}\u{884C}"; // 步行 (mode word, NOT a place)
const HONGSHULIN: &str = "\u{7D05}\u{6A39}\u{6797}"; // 紅樹林 (TW)
const NAHA_AIRPORT: &str = "\u{90A3}\u{8987}\u{6A5F}\u{5834}"; // 那覇機場 (JP)
const MONORAIL_STN: &str = "\u{55AE}\u{8ECC}\u{6CBF}\u{7DDA}\u{67D0}\u{7AD9}"; // 單軌沿線某站

// ── single: a junk mode-only stop ('步行') is REJECTED, nothing written ──────
#[test]
fn set_route_segment_rejects_mode_only_stop() {
    let tag = nanos();
    let plan_id = format!("test-rsg-junk-{tag}");
    let dest = format!("rsgjunk_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown_plan(&plan_id, &dest)
    });
    if !seed_plan_with_day(&plan_id, &dest) {
        return;
    }

    // from = 步行 (a transport WORD standing as a place) → must be rejected.
    let (ok, stdout, stderr) =
        run_cmd(&plan_id, &["set-route-segment", "1", "0", WALK, IIAS, "driving", "--dest", &dest]);

    let seg = seg_count(&plan_id);
    let audit = completed_audit_count(&plan_id, "set-route-segment");

    assert!(!ok, "a mode-only stop must exit non-zero; stdout={stdout} stderr={stderr}");
    assert_eq!(seg, Some(0), "NOTHING may be written when a stop is junk");
    assert_eq!(audit, Some(0), "no completed audit row when the guard rejects");
}

// ── single: an empty-after-clean stop is REJECTED ───────────────────────────
#[test]
fn set_route_segment_rejects_empty_after_clean() {
    let tag = nanos();
    let plan_id = format!("test-rsg-empty-{tag}");
    let dest = format!("rsgempty_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown_plan(&plan_id, &dest)
    });
    if !seed_plan_with_day(&plan_id, &dest) {
        return;
    }
    // （單軌約2分）+步行 cleans to nothing — no usable place name.
    let junk = "\u{FF08}\u{55AE}\u{8ECC}\u{7D04}2\u{5206}\u{FF09}+\u{6B65}\u{884C}";
    let (ok, _stdout, _stderr) =
        run_cmd(&plan_id, &["set-route-segment", "1", "0", AKAMINE_EKI, junk, "transit", "--dest", &dest]);
    let seg = seg_count(&plan_id);
    assert!(!ok, "an empty-after-clean stop must be rejected");
    assert_eq!(seg, Some(0), "NOTHING may be written");
}

// ── single: a bare clock-time stop is REJECTED ──────────────────────────────
#[test]
fn set_route_segment_rejects_clock_time_stop() {
    let tag = nanos();
    let plan_id = format!("test-rsg-clock-{tag}");
    let dest = format!("rsgclock_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown_plan(&plan_id, &dest)
    });
    if !seed_plan_with_day(&plan_id, &dest) {
        return;
    }
    let (ok, _stdout, _stderr) =
        run_cmd(&plan_id, &["set-route-segment", "1", "0", "08:30", IIAS, "driving", "--dest", &dest]);
    let seg = seg_count(&plan_id);
    assert!(!ok, "a bare clock-time stop must be rejected");
    assert_eq!(seg, Some(0), "NOTHING may be written");
}

// ── single: a cross-country (TW↔Okinawa) ground leg is REJECTED ─────────────
#[test]
fn set_route_segment_rejects_cross_country_pair() {
    let tag = nanos();
    let plan_id = format!("test-rsg-xc-{tag}");
    let dest = format!("rsgxc_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown_plan(&plan_id, &dest)
    });
    if !seed_plan_with_day(&plan_id, &dest) {
        return;
    }
    // 紅樹林 (TW) → 那覇機場 (JP) by driving = an ocean-spanning ground route.
    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &["set-route-segment", "1", "0", HONGSHULIN, NAHA_AIRPORT, "driving", "--dest", &dest],
    );
    let seg = seg_count(&plan_id);
    assert!(!ok, "a TW→JP ground leg must be rejected; stdout={stdout} stderr={stderr}");
    assert_eq!(seg, Some(0), "NOTHING may be written for a cross-country leg");
}

// ── single: a walking leg that rides rail is REJECTED ───────────────────────
#[test]
fn set_route_segment_rejects_walking_over_rail() {
    let tag = nanos();
    let plan_id = format!("test-rsg-walkrail-{tag}");
    let dest = format!("rsgwalkrail_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown_plan(&plan_id, &dest)
    });
    if !seed_plan_with_day(&plan_id, &dest) {
        return;
    }
    // mode=walking but a stop names 單軌 (monorail) → a walk over a rail leg.
    let (ok, _stdout, _stderr) = run_cmd(
        &plan_id,
        &["set-route-segment", "1", "0", AKAMINE_EKI, MONORAIL_STN, "walking", "--dest", &dest],
    );
    let seg = seg_count(&plan_id);
    assert!(!ok, "a walking leg over rail must be rejected");
    assert_eq!(seg, Some(0), "NOTHING may be written");
}

// ── single: a normal 単軌-area segment (赤嶺駅→iias, taxi) PASSES + persists ──
#[test]
fn set_route_segment_passes_normal_segment() {
    let tag = nanos();
    let plan_id = format!("test-rsg-ok-{tag}");
    let dest = format!("rsgok_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown_plan(&plan_id, &dest)
    });
    if !seed_plan_with_day(&plan_id, &dest) {
        return;
    }
    // 赤嶺駅 → iias 沖縄豊崎, driving (taxi): clean, same-country, not walking.
    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &["set-route-segment", "1", "0", AKAMINE_EKI, IIAS, "driving", "--dest", &dest],
    );
    let seg = seg_count(&plan_id);
    let audit = completed_audit_count(&plan_id, "set-route-segment");
    assert!(ok, "a clean segment must pass; stdout={stdout} stderr={stderr}");
    assert_eq!(seg, Some(1), "the clean segment must persist exactly one row");
    assert_eq!(audit, Some(1), "a clean write must record a completed audit row");
    let source = db_exec(&format!(
        "SELECT source AS v FROM day_route_segments \
         WHERE plan_id = '{plan_id}' AND destination = '{dest}' \
           AND day_number = 1 AND sort_order = 0"
    ));
    assert_eq!(
        source.and_then(|r| r.scalar()),
        Some("confirmed".to_string()),
        "normal set-route-segment must write source=confirmed"
    );
}

// ── single: --recommended writes source=ai_recommended ───────────────────────
#[test]
fn set_route_segment_passes_recommended_segment() {
    let tag = nanos();
    let plan_id = format!("test-rsg-rec-{tag}");
    let dest = format!("rsgrec_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown_plan(&plan_id, &dest)
    });
    if !seed_plan_with_day(&plan_id, &dest) {
        return;
    }
    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &[
            "set-route-segment",
            "1",
            "0",
            AKAMINE_EKI,
            IIAS,
            "driving",
            "--recommended",
            "--dest",
            &dest,
        ],
    );
    assert!(ok, "a clean --recommended segment must pass; stdout={stdout} stderr={stderr}");
    let source = db_exec(&format!(
        "SELECT source AS v FROM day_route_segments \
         WHERE plan_id = '{plan_id}' AND destination = '{dest}' \
           AND day_number = 1 AND sort_order = 0"
    ));
    assert_eq!(
        source.and_then(|r| r.scalar()),
        Some("ai_recommended".to_string()),
        "--recommended set-route-segment must write source=ai_recommended"
    );
}

// ── bulk: one bad segment rejects the WHOLE batch — no half-write ────────────
#[test]
fn set_route_segments_bulk_rejects_whole_batch_on_one_bad() {
    let tag = nanos();
    let plan_id = format!("test-rsg-bulk-{tag}");
    let dest = format!("rsgbulk_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown_plan(&plan_id, &dest)
    });
    if !seed_plan_with_day(&plan_id, &dest) {
        return;
    }
    // 2 good segments + 1 cross-country bad one → the WHOLE batch is rejected,
    // so day_route_segments stays empty (no partial half-write of the 2 good).
    let good1 = format!("{AKAMINE_EKI}|{IIAS}|driving");
    let bad = format!("{HONGSHULIN}|{NAHA_AIRPORT}|driving"); // TW→JP
    let good2 = format!("{IIAS}|{AKAMINE_EKI}|transit");
    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &[
            "set-route-segments-bulk", "1",
            "--seg", &good1, "--seg", &bad, "--seg", &good2,
            "--dest", &dest,
        ],
    );
    let seg = seg_count(&plan_id);
    let audit = completed_audit_count(&plan_id, "set-route-segments-bulk");
    assert!(!ok, "a batch with any bad segment must exit non-zero; stdout={stdout} stderr={stderr}");
    assert!(
        stderr.contains("--seg #1"),
        "the error must name the offending segment index; stderr={stderr}"
    );
    assert_eq!(seg, Some(0), "a rejected batch must NOT half-write the good segments");
    assert_eq!(audit, Some(0), "no completed audit row for a rejected batch");
}

// ── bulk: an all-clean batch PASSES and persists every segment ──────────────
#[test]
fn set_route_segments_bulk_passes_all_clean() {
    let tag = nanos();
    let plan_id = format!("test-rsg-bulkok-{tag}");
    let dest = format!("rsgbulkok_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown_plan(&plan_id, &dest)
    });
    if !seed_plan_with_day(&plan_id, &dest) {
        return;
    }
    let s1 = format!("{AKAMINE_EKI}|{IIAS}|driving");
    let s2 = format!("{IIAS}|{AKAMINE_EKI}|transit");
    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &["set-route-segments-bulk", "1", "--seg", &s1, "--seg", &s2, "--dest", &dest],
    );
    let seg = seg_count(&plan_id);
    assert!(ok, "an all-clean batch must pass; stdout={stdout} stderr={stderr}");
    assert_eq!(seg, Some(2), "both clean segments must persist");
    let sources = db_exec(&format!(
        "SELECT sort_order || '|' || source AS v FROM day_route_segments \
         WHERE plan_id = '{plan_id}' AND destination = '{dest}' AND day_number = 1 \
         ORDER BY sort_order"
    ));
    assert_eq!(
        sources.map(|r| r.column()),
        Some(vec![
            "0|confirmed".to_string(),
            "1|confirmed".to_string(),
        ]),
        "normal bulk must write source=confirmed on every segment"
    );
}

// ── bulk: --recommended writes source=ai_recommended on every segment ────────
#[test]
fn set_route_segments_bulk_passes_recommended() {
    let tag = nanos();
    let plan_id = format!("test-rsg-bulkrec-{tag}");
    let dest = format!("rsgbulkrec_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown_plan(&plan_id, &dest)
    });
    if !seed_plan_with_day(&plan_id, &dest) {
        return;
    }
    let s1 = format!("{AKAMINE_EKI}|{IIAS}|driving");
    let s2 = format!("{IIAS}|{AKAMINE_EKI}|transit");
    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &[
            "set-route-segments-bulk",
            "1",
            "--seg",
            &s1,
            "--seg",
            &s2,
            "--recommended",
            "--dest",
            &dest,
        ],
    );
    assert!(ok, "an all-clean --recommended batch must pass; stdout={stdout} stderr={stderr}");
    let sources = db_exec(&format!(
        "SELECT sort_order || '|' || source AS v FROM day_route_segments \
         WHERE plan_id = '{plan_id}' AND destination = '{dest}' AND day_number = 1 \
         ORDER BY sort_order"
    ));
    assert_eq!(
        sources.map(|r| r.column()),
        Some(vec![
            "0|ai_recommended".to_string(),
            "1|ai_recommended".to_string(),
        ]),
        "--recommended bulk must write source=ai_recommended on every segment"
    );
}