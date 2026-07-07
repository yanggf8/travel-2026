// Unknown-flag rejection locks for the four manual tour/offer catalog commands.
// All parse-before-connect (all-flags parsers), so a typo'd flag fails loud at
// parse — hermetic, no DB needed. A dropped --source/--note would silently lose
// provenance on a global catalog row.
mod common;
use common::bin;
use std::process::Command;

fn rejects(args: &[&str], bad: &str) {
    let out = Command::new(bin()).args(args).output().expect("run travel");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "should reject {bad}; err={err}");
    assert!(
        err.contains(&format!("unknown argument: {bad}")),
        "err should name {bad}; err={err}"
    );
}

#[test]
fn add_offer_rejects_unknown_flag() {
    rejects(
        &["add-offer", "--run", "r1", "--kind", "package", "--sourc", "x"],
        "--sourc",
    );
}

#[test]
fn add_besttour_offer_rejects_unknown_flag() {
    rejects(
        &["add-besttour-offer", "--url", "u", "--price", "1", "--hotl", "H"],
        "--hotl",
    );
}

#[test]
fn add_lifetour_offer_rejects_unknown_flag() {
    rejects(
        &["add-lifetour-offer", "--url", "u", "--price", "1", "--noote", "n"],
        "--noote",
    );
}

#[test]
fn import_tour_group_offers_rejects_unknown_flag() {
    rejects(
        &["import-tour-group-offers", "--run", "r1", "--fil", "p"],
        "--fil",
    );
}
