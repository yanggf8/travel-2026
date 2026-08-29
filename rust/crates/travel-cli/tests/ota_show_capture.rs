//! Integration tests for `travel ota show-capture`.
//! Help / missing-id / unknown-subcommand need no Turso. The live capture dump
//! skips cleanly if creds are absent. Captures are not plan-keyed — teardown
//! DELETEs the test capture_id explicitly.

use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{bin, db_exec, db_exec_teardown, is_credless, nanos, Guard};

static SHOW_CAPTURE_LOCK: Mutex<()> = Mutex::new(());

fn run(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn teardown_capture(capture_id: &str) {
    let _ = db_exec_teardown(&format!(
        "DELETE FROM captures WHERE capture_id='{capture_id}'"
    ));
}

#[test]
fn show_capture_help_prints_usage() {
    for flag in ["--help", "-h"] {
        let (ok, stdout, stderr) = run(&["ota", "show-capture", flag]);
        let combined = format!("{stdout}{stderr}");
        assert!(ok, "show-capture {flag} should exit 0; stderr={stderr}");
        assert!(
            combined.contains("Usage"),
            "show-capture {flag} must print Usage; got: {combined}"
        );
    }
}

#[test]
fn show_capture_missing_id_fails_loud() {
    let (ok, _stdout, stderr) = run(&["ota", "show-capture"]);
    assert!(!ok, "missing capture_id must fail");
    assert!(
        stderr.contains("Usage"),
        "missing capture_id must print Usage; stderr={stderr}"
    );
}

#[test]
fn unknown_ota_subcommand_usage_lists_show_capture() {
    let (ok, _stdout, stderr) = run(&["ota", "no-such-sub"]);
    assert!(!ok);
    assert!(
        stderr.contains("show-capture"),
        "unknown ota subcommand Usage must list show-capture; stderr={stderr}"
    );
}

#[tokio::test]
async fn show_capture_prints_raw_text_and_stderr_summary() {
    let _lock = SHOW_CAPTURE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }

    let suffix = nanos();
    let capture_id = format!("test-showcap-{suffix}");
    teardown_capture(&capture_id);
    let _g = Guard::new({
        let capture_id = capture_id.clone();
        move || teardown_capture(&capture_id)
    });

    let raw = "LINE1 offer $12,345\nLINE2 hotel 東京\n\"quoted\" and \ttab\n";
    let now = "2026-08-26T10:00:00Z";
    let url = "https://example.com/ota-page";
    db_exec(&format!(
        "INSERT INTO captures (capture_id, source_id, url, captured_at, raw_text) \
         VALUES ('{capture_id}', 'zzshow', '{url}', '{now}', '{raw}')"
    ))
    .expect("seed capture");

    let (ok, stdout, stderr) = run(&["ota", "show-capture", &capture_id]);
    assert!(ok, "show-capture failed: {stderr}");
    assert_eq!(stdout, raw, "raw_text must be printed verbatim to stdout");
    assert!(
        !stdout.trim_start().starts_with('{'),
        "must not wrap raw_text in JSON"
    );
    assert!(
        stderr.contains("source_id=zzshow"),
        "stderr summary must include source_id; stderr={stderr}"
    );
    assert!(
        stderr.contains(&format!("url={url}")),
        "stderr summary must include url; stderr={stderr}"
    );
    assert!(
        stderr.contains(&format!("captured_at={now}")),
        "stderr summary must include captured_at; stderr={stderr}"
    );

    let (ok, stdout, stderr) = run(&["ota", "show_capture", &capture_id]);
    assert!(ok, "show_capture alias failed: {stderr}");
    assert_eq!(stdout, raw, "alias must print the same raw_text");

    let (ok, _stdout, stderr) = run(&["ota", "show-capture", "test-showcap-missing-no-such"]);
    assert!(!ok, "missing capture must exit 1");
    assert!(
        stderr.contains("not found"),
        "missing capture must fail loud; stderr={stderr}"
    );
}
