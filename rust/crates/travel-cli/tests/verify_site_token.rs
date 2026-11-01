//! Real-Turso integration test for `travel verify-site --provision-token [--rotate]`.
//!
//! Locks the verify-token contract (docs/plans/2026-10-30-verify-site.md, 第 1 層):
//! at most one ACTIVE token per plan with created_by='verify-site'; provisioning
//! is idempotent (reuses it); --rotate deactivates it and mints a new one; a
//! share-token ('cli') token of the same plan is never touched; no full token is
//! ever printed; no plan audit triad (plans.version unchanged). Seeds a
//! throwaway zztest plan, tears it down through the canonical teardown_plan
//! (plan_share_tokens is plan_id-keyed). Skips cleanly without Turso creds.

use std::process::Command;

mod common;
use common::{bin, db_exec, is_credless, nanos, seed_plan, teardown_plan, Guard};

fn run(args: &[&str]) -> (bool, String, String) {
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

fn scalar(sql: &str) -> String {
    db_exec(sql).expect("creds checked").scalar().unwrap_or_default()
}

fn tokens(plan: &str) -> Vec<String> {
    db_exec(&format!("SELECT token FROM plan_share_tokens WHERE plan_id = '{plan}'"))
        .expect("creds checked")
        .column()
}

fn assert_no_full_token(plan: &str, out: &str) {
    for t in tokens(plan) {
        assert!(!out.contains(&t), "a full token was printed: {out}");
    }
}

#[test]
fn provision_token_is_idempotent_and_rotate_replaces_it() {
    if db_exec("SELECT 1 AS n").is_none() {
        eprintln!("skipping verify-site token test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zztest{tag}vs");
    let dest = format!("zztest_dest_{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_plan(&plan, &dest, 0);

    let active_verify = || {
        scalar(&format!(
            "SELECT COUNT(*) AS n FROM plan_share_tokens WHERE plan_id = '{plan}' \
             AND created_by = 'verify-site' AND status = 'active'"
        ))
    };

    // A share-token ('cli') link that verify-site must leave alone.
    let (ok, _, stderr) = run(&["share-token", "--plan-id", &plan]);
    if !ok && is_credless(&stderr) {
        eprintln!("skipping verify-site token test (no Turso creds): {}", stderr.trim());
        return;
    }
    assert!(ok, "share-token: {stderr}");

    // --provision-token without a plan is refused before any write.
    let (ok, _, stderr) = run(&["verify-site", "--provision-token"]);
    assert!(!ok && stderr.contains("requires --plan-id"), "{stderr}");

    // 1. First provision mints one verify token.
    let (ok, stdout, stderr) = run(&["verify-site", "--plan-id", &plan, "--provision-token"]);
    assert!(ok, "provision: {stdout} {stderr}");
    assert!(stdout.contains("minted:") && stdout.contains("created_by=verify-site"), "{stdout}");
    assert!(stdout.contains(&format!("next: travel verify-site --plan-id {plan}")), "{stdout}");
    assert_no_full_token(&plan, &format!("{stdout}{stderr}"));
    assert_eq!(active_verify(), "1");
    let first = scalar(&format!(
        "SELECT token FROM plan_share_tokens WHERE plan_id = '{plan}' AND created_by = 'verify-site'"
    ));

    // 2. Second provision reuses it: no new row.
    let (ok, stdout, stderr) = run(&["verify-site", "--plan-id", &plan, "--provision-token"]);
    assert!(ok, "re-provision: {stdout} {stderr}");
    assert!(stdout.contains("reused:") && !stdout.contains("minted:"), "{stdout}");
    assert!(stdout.contains(&first[..6]), "the reused fingerprint: {stdout}");
    assert_no_full_token(&plan, &format!("{stdout}{stderr}"));
    assert_eq!(
        scalar(&format!(
            "SELECT COUNT(*) AS n FROM plan_share_tokens WHERE plan_id = '{plan}' AND created_by = 'verify-site'"
        )),
        "1"
    );

    // 3. --rotate deactivates it and mints a fresh one.
    let (ok, stdout, stderr) = run(&["verify-site", "--plan-id", &plan, "--provision-token", "--rotate"]);
    assert!(ok, "rotate: {stdout} {stderr}");
    assert!(stdout.contains("deactivated:") && stdout.contains("minted:"), "{stdout}");
    assert_no_full_token(&plan, &format!("{stdout}{stderr}"));
    assert_eq!(active_verify(), "1");
    assert_eq!(
        scalar(&format!(
            "SELECT status || '/' || deactivated_by AS v FROM plan_share_tokens WHERE token = '{first}'"
        )),
        "inactive/verify-site"
    );

    // The share-token link is untouched, and nothing bumped the plan version.
    assert_eq!(
        scalar(&format!(
            "SELECT COUNT(*) AS n FROM plan_share_tokens WHERE plan_id = '{plan}' AND created_by = 'cli' AND status = 'active'"
        )),
        "1"
    );
    assert_eq!(scalar(&format!("SELECT version FROM plans WHERE plan_id = '{plan}'")), "0");
}

#[test]
fn provision_token_refuses_an_unknown_plan() {
    if db_exec("SELECT 1 AS n").is_none() {
        eprintln!("skipping verify-site token test (no Turso creds)");
        return;
    }
    let plan = format!("zztest{}missing", nanos());
    // Nothing is seeded; the guard only catches a regression that would write.
    let _g = Guard::new({
        let plan = plan.clone();
        move || teardown_plan(&plan, "")
    });
    let (ok, _, stderr) = run(&["verify-site", "--plan-id", &plan, "--provision-token"]);
    if !ok && is_credless(&stderr) {
        return;
    }
    assert!(!ok && stderr.contains("not found or soft-deleted"), "{stderr}");
    assert_eq!(scalar(&format!("SELECT COUNT(*) AS n FROM plan_share_tokens WHERE plan_id = '{plan}'")), "0");
}
