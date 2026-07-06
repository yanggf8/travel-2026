mod common;
use common::bin;
use std::process::Command;

#[test]
fn help_prints_usage() {
    let out = Command::new(bin())
        .args(["compare", "content-depth", "--help"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("Usage:"), "stdout: {s}");
    assert!(
        s.contains("travel compare content-depth --plan-id"),
        "stdout: {s}"
    );
    assert!(
        s.contains("okinawa-2026"),
        "help should name the default reference; stdout: {s}"
    );
}