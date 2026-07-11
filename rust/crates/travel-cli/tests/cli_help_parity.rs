mod common;
use common::bin;
use std::process::Command;

fn run(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin()).args(args).output().expect("run travel");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn help_prints_usage_for_query_commands() {
    for cmd in ["query-offers", "query-destination-ref", "query-bookings", "check-freshness"] {
        for flag in ["--help", "-h"] {
            let (ok, stdout, stderr) = run(&[cmd, flag]);
            let combined = format!("{stdout}{stderr}");
            assert!(ok, "{cmd} {flag} should exit 0; stderr={stderr}");
            assert!(
                combined.contains("Usage") || combined.contains("usage"),
                "{cmd} {flag} should print Usage; got: {combined}"
            );
            assert!(
                !combined.contains("unknown flag"),
                "{cmd} {flag} must NOT say 'unknown flag'; got: {combined}"
            );
        }
    }
}

#[test]
fn real_typo_flag_still_errors() {
    // A genuine unknown flag must still fail loud (we only intercept --help/-h).
    let (ok, _stdout, stderr) = run(&["query-offers", "--totally-bogus-flag"]);
    assert!(!ok, "a real unknown flag must still error");
    assert!(
        stderr.contains("unknown flag") || stderr.contains("Error"),
        "a real unknown flag must fail loud; stderr={stderr}"
    );
}

#[test]
fn populate_missing_goals_points_to_cluster_list() {
    // The --goals error is emitted by parse_args before any Turso connect, so
    // no creds / no seed needed. An explicit --plan-id avoids plan ambiguity.
    let (ok, _stdout, stderr) = run(&["populate-itinerary", "--plan-id", "no-such-plan-xyz"]);
    assert!(!ok, "missing --goals must error (exit non-zero)");
    assert!(
        stderr.contains("requires --goals"),
        "expected the --goals error; got: {stderr}"
    );
    assert!(
        stderr.contains("query-destination-ref"),
        "the --goals error must point to query-destination-ref; got: {stderr}"
    );
}
