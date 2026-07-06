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

#[test]
fn missing_plan_id_fails() {
    let out = Command::new(bin())
        .args(["compare", "content-depth", "--against", "okinawa-2026"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let s = String::from_utf8_lossy(&out.stderr);
    assert!(s.contains("--plan-id"), "stderr: {s}");
}

#[test]
fn unknown_flag_fails() {
    let out = Command::new(bin())
        .args([
            "compare",
            "content-depth",
            "--plan-id",
            "x-2026",
            "--bogus",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let s = String::from_utf8_lossy(&out.stderr);
    assert!(
        s.to_lowercase().contains("unknown flag"),
        "stderr: {s}"
    );
}