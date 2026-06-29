//! Regression test for demoting CLAUDE.md OTA status facts to a CLI pointer.

use std::fs;
use std::process::Command;

mod common;
use common::Guard;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_travel"))
}

fn is_credless(stderr: &str) -> bool {
    stderr.contains("turso auth login")
        || stderr.contains("Missing Turso")
        || stderr.contains("failed to connect to Turso")
        || stderr.contains("TRAVEL_TURSO")
}

fn run(args: &[&str]) -> (bool, String, String) {
    let out = bin()
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[tokio::test]
async fn claude_md_points_to_ota_status_and_validate_data_passes() {
    let _g = Guard::new(|| {});
    let claude_path = format!("{}/../../../CLAUDE.md", env!("CARGO_MANIFEST_DIR"));
    let content = fs::read_to_string(claude_path).expect("read CLAUDE.md");
    let pointer = "Provider coverage is DB data — run `travel ota-status` (catalog edited via `travel set-ota-*`).";
    assert!(
        content.contains(pointer),
        "CLAUDE.md must contain the OTA coverage pointer"
    );

    let section = content
        .split("## OTA Sources")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .unwrap_or("");
    assert!(
        !section.contains("| Source ID |"),
        "OTA section must not contain a status table"
    );
    for forbidden in [
        "PROVEN REAL",
        "DEFERRED",
        "renderer-wedge",
        "cloudflare",
        "captcha",
    ] {
        assert!(
            !section.contains(forbidden),
            "OTA section must not encode status fact {forbidden}"
        );
    }

    let (ok, stdout, stderr) = run(&["validate", "data"]);
    if !ok && is_credless(&stderr) {
        eprintln!("skipping (no creds): {}", stderr.trim());
        return;
    }
    assert!(
        ok,
        "validate data should pass; stdout={stdout} stderr={stderr}"
    );
}
