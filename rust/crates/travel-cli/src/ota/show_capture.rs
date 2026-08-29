//! `travel ota show-capture <capture_id>` — read-only dump of `captures.raw_text`.
//!
//! Alias: `show_capture`. The coding agent is the parser; this command is the CLI
//! surface that prints the capture so the agent does not have to `db exec` for
//! raw_text. Stdout is the text as stored (no JSON wrap, no truncation, no extra
//! newline). A one-line source_id/url/captured_at summary goes to stderr.
//! Does not write the audit triad.

use crate::db;
use travel_db::repo::captures;

const USAGE: &str = "Usage: travel ota show-capture <capture_id>\n       travel ota show_capture <capture_id>";

pub async fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return Ok(());
    }

    let mut capture_id: Option<&str> = None;
    for a in args {
        if a.starts_with('-') {
            return Err(format!("unknown flag for show-capture: {a}\n{USAGE}"));
        }
        if capture_id.is_some() {
            return Err(USAGE.to_string());
        }
        capture_id = Some(a.as_str());
    }
    let capture_id = capture_id.ok_or_else(|| USAGE.to_string())?;

    let conn = db::connect_read().await?;
    let cap = captures::get(&conn, capture_id)
        .await?
        .ok_or_else(|| format!("Error: capture_id '{capture_id}' not found"))?;

    let url = cap.url.as_deref().unwrap_or("-");
    let captured_at = if cap.captured_at.is_empty() {
        "-"
    } else {
        cap.captured_at.as_str()
    };
    eprintln!(
        "source_id={} url={} captured_at={}",
        cap.source_id, url, captured_at
    );
    print!("{}", cap.raw_text);
    Ok(())
}
