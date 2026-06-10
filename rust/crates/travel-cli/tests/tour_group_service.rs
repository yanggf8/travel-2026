//! Port of tests/integration/tour-group-service.regression.test.ts.
//!
//! The TS test asserts the service module exports its public API
//! (insertTourGroupOffers / upsertScrapeAttempt / listTourGroupOffers /
//! findAuditSet). In the Rust CLI there is no importable service module — the
//! equivalent surface is the subcommands — so this is a minimal smoke test that
//! the tour-group commands dispatch (help returns 0 and prints usage).

#[tokio::test]
async fn tour_group_commands_dispatch() {
    let bin = env!("CARGO_BIN_EXE_travel");

    for (cmd, needle) in [
        ("query-tour-group-offers", "query-tour-group-offers"),
        ("import-tour-group-offers", "import-tour-group-offers"),
    ] {
        let out = std::process::Command::new(bin)
            .args([cmd, "--help"])
            .output()
            .unwrap_or_else(|e| panic!("run {cmd} --help: {e}"));
        assert!(out.status.success(), "{cmd} --help should exit 0");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains(needle) || stdout.to_lowercase().contains("usage"),
            "{cmd} --help should print usage; got: {stdout}"
        );
    }
}
