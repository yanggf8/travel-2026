//! Real-Turso integration test for verify-site's per-run verify token.
//!
//! Contract (docs/plans/2026-10-30-verify-site.md, 第 1 層): a run mints a
//! verify token (created_by='verify-site') only for the moment it opens the
//! viewer page and DELETES it afterwards — whatever the checks found; a run
//! first removes verify tokens left behind by an interrupted run; a
//! share-token ('cli') row is never touched; no full token is ever printed; no
//! plan audit triad (plans.version unchanged). Seeds a throwaway zztest plan,
//! tears it down through the canonical teardown_plan (plan_share_tokens is
//! plan_id-keyed). Skips cleanly without Turso creds.

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

#[test]
fn verify_token_lives_only_for_the_run_and_leftovers_are_removed() {
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

    // A leftover verify token of an "interrupted run", and a share-token link
    // that verify-site must leave alone.
    let head = (tag % 0xff_fff0) as u32;
    let leftover = format!("{:06x}{:026x}", head, tag);
    let link = format!("{:06x}{:026x}", head + 1, tag);
    for (t, by) in [(&leftover, "verify-site"), (&link, "cli")] {
        db_exec(&format!(
            "INSERT INTO plan_share_tokens (plan_id, token, created_at, status, created_by) \
             VALUES ('{plan}', '{t}', datetime('now'), 'active', '{by}')"
        ));
    }

    // The checks themselves may FAIL for a bare test plan; only the token
    // lifecycle is asserted here.
    let (ok, stdout, stderr) = run(&["verify-site", "--plan-id", &plan]);
    if !ok && is_credless(&stderr) {
        eprintln!("skipping verify-site token test (no Turso creds): {}", stderr.trim());
        return;
    }
    let out = format!("{stdout}{stderr}");
    // The run must actually have reached layer 1 (mint → page checks → delete);
    // an abort before the mint would leave every token assertion below green.
    assert!(
        stdout.lines().any(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            f.len() > 3 && f[1] == plan && f[2] == "page" && f[3] == "zh"
        }),
        "layer 1 never ran for the plan: {out}"
    );
    assert!(out.contains("leftover verify token"), "the sweep reports what it removed: {out}");
    assert!(!out.contains(&leftover) && !out.contains(&link), "a full token was printed: {out}");
    assert!(!out.contains("WARN token"), "no 'no verify token' skip any more: {out}");

    assert_eq!(
        scalar(&format!(
            "SELECT COUNT(*) AS n FROM plan_share_tokens WHERE plan_id = '{plan}' AND created_by = 'verify-site'"
        )),
        "0",
        "the run's own token and the leftover are both gone"
    );
    assert_eq!(
        scalar(&format!("SELECT COUNT(*) AS n FROM plan_share_tokens WHERE token = '{link}'")),
        "1",
        "a share-token link is never touched"
    );
    assert_eq!(scalar(&format!("SELECT version FROM plans WHERE plan_id = '{plan}'")), "0");

    // The provisioning flags are gone: the token is per run.
    let (ok, _, stderr) = run(&["verify-site", "--plan-id", &plan, "--provision-token"]);
    assert!(!ok && stderr.contains("--provision-token"), "{stderr}");
}

#[test]
fn verify_site_refuses_an_unknown_plan_without_minting() {
    if db_exec("SELECT 1 AS n").is_none() {
        eprintln!("skipping verify-site token test (no Turso creds)");
        return;
    }
    let plan = format!("zztest{}missing", nanos());
    let _g = Guard::new({
        let plan = plan.clone();
        move || teardown_plan(&plan, "")
    });
    let (ok, _, stderr) = run(&["verify-site", "--plan-id", &plan]);
    if !ok && is_credless(&stderr) {
        return;
    }
    assert!(!ok && stderr.contains("not found or soft-deleted"), "{stderr}");
    assert_eq!(scalar(&format!("SELECT COUNT(*) AS n FROM plan_share_tokens WHERE plan_id = '{plan}'")), "0");
}
