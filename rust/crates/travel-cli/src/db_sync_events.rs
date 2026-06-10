//! `db sync events` — port of scripts/turso-sync-events.ts.
//!
//! Reads `data/state.json` (legacy file-based event log), hashes each event's
//! content into a stable `external_id`, and idempotently inserts rows into the
//! Turso `events` table via `ON CONFLICT(external_id) DO NOTHING`.
//!
//! Idempotency optimization (mirrors TS): query `MAX(created_at)` for events
//! that did NOT come from a prior turso import, and only send events with
//! `at >= lastSyncedAt` (still guarded by ON CONFLICT).
//!
//! Notes vs. the TS source:
//!   * The live `events` table column is `data_text` (the de-JSON program
//!     renamed the old `data` column). We write the serialized payload there.
//!   * If `data/state.json` is absent the TS script prints to stderr and exits
//!     1; we mirror that by returning an error.
//!
//! Signature fixed by main.rs dispatch: `run(&[String])`.

use crate::db::connect_write;
use sha1::{Digest, Sha1};
use std::path::PathBuf;

pub async fn run(args: &[String]) -> Result<(), String> {
    let _ = args;

    let state_path = PathBuf::from(std::env::current_dir().map_err(|e| e.to_string())?)
        .join("data/state.json");

    if !state_path.exists() {
        // TS: console.error(...) + process.exit(1)
        return Err(format!("State file not found: {}", state_path.display()));
    }

    let raw = std::fs::read_to_string(&state_path)
        .map_err(|e| format!("failed to read {}: {e}", state_path.display()))?;
    let state: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("failed to parse state.json: {e}"))?;

    let empty: Vec<serde_json::Value> = Vec::new();
    let event_log: &Vec<serde_json::Value> = state
        .get("event_log")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);

    if event_log.is_empty() {
        println!("No events in event log.");
        return Ok(());
    }

    let conn = connect_write().await?;

    // Idempotency: external_id (hash of event content) prevents duplicates.
    println!("Preparing to sync {} events...", event_log.len());

    // Optimization: only send events newer than the last known synced event.
    // Falls back to all events on any error (mirrors the TS try/catch).
    let mut to_sync: Vec<&serde_json::Value> = event_log.iter().collect();
    match last_synced_at(&conn).await {
        Ok(Some(last_at)) => {
            println!("Last synced event in Turso: {last_at}");
            to_sync = event_log
                .iter()
                .filter(|e| event_at(e).map(|at| at >= last_at).unwrap_or(true))
                .collect();
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!("Could not check last synced event, syncing all: {e}");
        }
    }

    if to_sync.is_empty() {
        println!("All events are already synced.");
        return Ok(());
    }

    println!("Syncing {} events (idempotent)...", to_sync.len());

    for event in &to_sync {
        let eid = event_id(event);
        let event_type = str_field(event, "event");
        let destination = opt_str_field(event, "destination");
        let process = opt_str_field(event, "process");
        let data_text = event
            .get("data")
            .filter(|v| !v.is_null())
            .map(|v| serde_json::to_string(v).unwrap_or_default());
        let created_at = str_field(event, "at");

        conn.execute(
            "INSERT INTO events (external_id, event_type, destination, process, data_text, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(external_id) DO NOTHING",
            libsql::params![
                eid,
                event_type,
                opt_param(destination),
                opt_param(process),
                opt_param(data_text),
                created_at,
            ],
        )
        .await
        .map_err(|e| format!("events INSERT failed: {e}"))?;
    }

    println!("✅ Events sync complete.");
    Ok(())
}

/// Query MAX(created_at) over events that did not come from a prior turso
/// import. Returns None if there is no prior synced event.
async fn last_synced_at(conn: &libsql::Connection) -> Result<Option<String>, String> {
    let mut rows = conn
        .query(
            "SELECT MAX(created_at) AS v FROM events WHERE process != 'turso_import'",
            libsql::params![],
        )
        .await
        .map_err(|e| format!("events MAX(created_at) query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("events MAX(created_at) row read failed: {e}"))?
    {
        // MAX over zero rows yields SQL NULL.
        let v: Option<String> = row.get(0).ok();
        return Ok(v.filter(|s| !s.is_empty()));
    }
    Ok(None)
}

/// Generate a unique ID for an event by SHA1-hashing its content, matching the
/// TS `JSON.stringify({at, event, destination, process, data})` key order.
/// Keys whose value is undefined (absent/null) are omitted, mirroring JS.
fn event_id(event: &serde_json::Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    for key in ["at", "event", "destination", "process", "data"] {
        if let Some(v) = event.get(key) {
            if v.is_null() {
                continue; // JSON.stringify omits undefined; treat null absence the same
            }
            let encoded = serde_json::to_string(v).unwrap_or_default();
            let encoded_key = serde_json::to_string(key).unwrap_or_default();
            parts.push(format!("{encoded_key}:{encoded}"));
        }
    }
    let payload = format!("{{{}}}", parts.join(","));
    let mut hasher = Sha1::new();
    hasher.update(payload.as_bytes());
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn event_at(event: &serde_json::Value) -> Option<String> {
    event.get("at").and_then(|v| v.as_str()).map(String::from)
}

fn str_field(event: &serde_json::Value, key: &str) -> String {
    event
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn opt_str_field(event: &serde_json::Value, key: &str) -> Option<String> {
    event
        .get(key)
        .filter(|v| !v.is_null())
        .and_then(|v| v.as_str())
        .map(String::from)
}

fn opt_param(value: Option<String>) -> libsql::Value {
    match value {
        Some(s) => libsql::Value::Text(s),
        None => libsql::Value::Null,
    }
}
