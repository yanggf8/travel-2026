//! Live smoke test for `travel verify-site`: runs the real command against the
//! real dashboard and real Turso. It reaches the network (HTTP GETs to the
//! dashboard worker), so it runs ONLY when TRAVEL_VERIFY_SITE_LIVE=1 and skips
//! otherwise — never part of an ordinary `make test`. Read-only: verify-site
//! writes nothing, so there is no fixture and no teardown.

use std::process::Command;

mod common;
use common::bin;

#[test]
fn verify_site_runs_against_the_live_dashboard() {
    if std::env::var("TRAVEL_VERIFY_SITE_LIVE").as_deref() != Ok("1") {
        eprintln!("skip: set TRAVEL_VERIFY_SITE_LIVE=1 to run the live verify-site smoke test");
        return;
    }
    let out = Command::new(bin())
        .arg("verify-site")
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .expect("run travel verify-site");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let summary = stdout
        .lines()
        .find(|l| l.starts_with("summary: "))
        .unwrap_or_else(|| panic!("no summary line\nstdout:\n{stdout}\nstderr:\n{stderr}"));
    assert!(
        summary.contains(" plan(s) · ") && summary.contains(" PASS · ") && summary.ends_with(" FAIL"),
        "malformed summary: {summary}"
    );
    // Every other stdout line is one verdict.
    for line in stdout.lines().filter(|l| !l.starts_with("summary: ")) {
        assert!(
            ["PASS ", "WARN ", "FAIL "].iter().any(|p| line.starts_with(p)),
            "non-verdict line: {line}"
        );
    }
    // Exit status tracks FAIL lines: non-zero iff any FAIL was printed.
    let fails = stdout.lines().filter(|l| l.starts_with("FAIL ")).count();
    assert_eq!(out.status.success(), fails == 0, "exit {:?} with {fails} FAIL line(s)", out.status);
}

#[test]
fn verify_site_rejects_unknown_flags_without_network() {
    let out = Command::new(bin())
        .args(["verify-site", "--bogus"])
        .output()
        .expect("run travel verify-site");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown argument: --bogus"));
}
