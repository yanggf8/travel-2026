//! Real-Turso integration tests for day variants (Task A1): `days.variant_of` /
//! `variant_label(_zh)` invariants enforced by `validate publish` (BLOCK).
//!
//! Harness mirrors tests/swap_days.rs: nanos-tagged throwaway plan, Guard armed right after the
//! plan id is bound, canonical teardown_plan. Skips cleanly when Turso creds are absent.

use std::process::Command;

mod common;
use common::{bin, db_exec, nanos, seed_plan, teardown_plan, Guard};

fn publish(plan_id: &str, dest: &str) -> (bool, String) {
    let out = Command::new(bin())
        .args(["validate", "publish", "--plan-id", plan_id, "--dest", dest])
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .unwrap_or_else(|e| panic!("run validate publish: {e}"));
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

fn variant_lines(out: &str) -> Vec<&str> {
    out.lines().filter(|l| l.contains("[variant]")).collect()
}

#[test]
fn variant_invariants_block_publish() {
    if db_exec("SELECT 1 AS n").is_none() {
        return;
    }
    let tag = nanos();
    let plan_id = format!("zz-test-{tag}-var");
    let dest = format!("zz_test_{tag}_var");
    seed_plan(&plan_id, &dest, 0);
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown_plan(&plan_id, &dest)
    });
    let ok = db_exec(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, theme, day_type, status) \
           VALUES ('{plan_id}', '{dest}', 1, '2026-09-01', 'T1', 'arrival', 'draft'); \
         INSERT INTO days (plan_id, destination, day_number, date, theme, day_type, status) \
           VALUES ('{plan_id}', '{dest}', 2, '2026-09-02', 'T2', 'departure', 'draft'); \
         INSERT INTO days (plan_id, destination, day_number, date, theme, day_type, status, variant_of) \
           VALUES ('{plan_id}', '{dest}', 3, '2026-09-05', 'V', 'arrival', 'draft', 1);"
    ))
    .is_some();
    assert!(ok, "seed days failed");

    // 1. date differs from main day's -> blocked, a line mentions the variant.
    let (success, out) = publish(&plan_id, &dest);
    assert!(!success, "date mismatch must block; out={out}");
    assert!(!variant_lines(&out).is_empty(), "expected a [variant] line; out={out}");
    assert!(out.contains("date"), "expected a date message; out={out}");

    // 2. date fixed, labels blank -> still blocked (labels).
    db_exec(&format!(
        "UPDATE days SET date='2026-09-01' WHERE plan_id='{plan_id}' AND day_number=3"
    ))
    .expect("fix date");
    let (success, out) = publish(&plan_id, &dest);
    assert!(!success, "blank labels must block; out={out}");
    assert!(out.contains("no variant_label"), "expected label message; out={out}");

    // 3. labels on variant AND main -> the variant category passes.
    db_exec(&format!(
        "UPDATE days SET variant_label='Sunny plan', variant_label_zh='晴天方案' \
           WHERE plan_id='{plan_id}' AND day_number=3; \
         UPDATE days SET variant_label='Rainy plan', variant_label_zh='雨天方案' \
           WHERE plan_id='{plan_id}' AND day_number=1;"
    ))
    .expect("set labels");
    let (_, out) = publish(&plan_id, &dest);
    assert!(
        variant_lines(&out).is_empty(),
        "valid variant must raise no [variant] line; out={out}"
    );

    // 4. variant_of pointing at a variant (chain) -> blocked.
    db_exec(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, theme, day_type, status, \
                           variant_of, variant_label, variant_label_zh) \
           VALUES ('{plan_id}', '{dest}', 4, '2026-09-01', 'V2', 'arrival', 'draft', 3, 'Chain', '鏈');"
    ))
    .expect("seed chain");
    let (success, out) = publish(&plan_id, &dest);
    assert!(!success, "chained variant must block; out={out}");
    assert!(out.contains("itself a variant"), "expected chain message; out={out}");
}
