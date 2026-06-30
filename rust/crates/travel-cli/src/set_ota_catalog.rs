// `travel set-ota-source` / `set-ota-coverage` / `set-ota-region` / `set-ota-workflow` /
// `set-ota-url-token` — audited mutations of the
// normalized OTA provider catalog (DB-centric provider architecture, spec 2026-06-29).
//
// These are the write surface that makes the DB the source of truth for the provider catalog
// (replacing "edit the OTA_SOURCES Rust array + migrate"). Each mutation SELECT-validates its
// references (product_type ∈ product_types, blocked ∈ coverage_block_reasons) fail-loud, then
// UPSERTs, then writes one catalog_runs audit row. Plain-text output only.

use crate::catalog_audit::record_catalog_run;
use crate::cascade::common::now_db_datetime;
use crate::db;
use libsql::Connection;

// ---------------------------------------------------------------------------
// set-ota-source <source_id> --name <n> --status active|inactive
// ---------------------------------------------------------------------------
pub async fn run_set_source(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Usage:\n  travel set-ota-source <source_id> --name <name> --status active|inactive");
        return Ok(());
    }
    let pos: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let source_id = pos
        .first()
        .ok_or_else(|| "Error: set-ota-source requires <source_id>".to_string())?
        .to_string();
    let name = opt_val(args, "--name");
    let status = opt_val(args, "--status");
    if let Some(ref s) = status {
        if s != "active" && s != "inactive" {
            return Err(format!("Error: --status must be active|inactive (got {s})"));
        }
    }

    let conn = db::connect_write().await?;
    let now = now_db_datetime();
    // UPSERT identity only (name/status). COALESCE keeps an existing value when a flag is
    // omitted, so a partial edit doesn't blank the other column.
    conn.execute(
        "INSERT INTO ota_sources (source_id, name, status, updated_at) \
         VALUES (?1, COALESCE(?2, ?1), COALESCE(?3, 'active'), ?4) \
         ON CONFLICT(source_id) DO UPDATE SET \
            name = COALESCE(?2, ota_sources.name), \
            status = COALESCE(?3, ota_sources.status), \
            updated_at = ?4",
        libsql::params![source_id.clone(), name.clone(), status.clone(), now],
    )
    .await
    .map_err(|e| e.to_string())?;

    record_catalog_run(&conn, "set-ota-source", &source_id).await?;
    println!("ota_sources upserted: {source_id}");
    Ok(())
}

// ---------------------------------------------------------------------------
// set-ota-coverage <source_id> <product_type> [--proven] [--proven-at <date>]
//                  [--method agent_parse|regex] [--search-url <u>] [--blocked <reason>]
// ---------------------------------------------------------------------------
pub async fn run_set_coverage(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Usage:\n  travel set-ota-coverage <source_id> <product_type> [--proven] [--proven-at YYYY-MM-DD] [--method agent_parse|regex] [--search-url <url>] [--blocked <reason_code>]");
        return Ok(());
    }
    let pos: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if pos.len() < 2 {
        return Err("Error: set-ota-coverage requires <source_id> <product_type>".to_string());
    }
    let source_id = pos[0].to_string();
    let product_type = pos[1].to_string();
    let proven = args.iter().any(|a| a == "--proven");
    let proven_at = opt_val(args, "--proven-at");
    let method = opt_val(args, "--method");
    let search_url = opt_val(args, "--search-url");
    let blocked = opt_val(args, "--blocked");

    // Enforce: proven ⇒ proven_at AND method (fail loud, write nothing).
    if proven && (proven_at.is_none() || method.is_none()) {
        return Err(
            "Error: --proven requires both --proven-at <date> and --method <agent_parse|regex>"
                .to_string(),
        );
    }
    if let Some(ref m) = method {
        if m != "agent_parse" && m != "regex" {
            return Err(format!("Error: --method must be agent_parse|regex (got {m})"));
        }
    }

    let conn = db::connect_write().await?;

    // SELECT-validate references (fail loud — no silent bad FK).
    if !exists(&conn, "product_types", "code", &product_type).await? {
        return Err(format!(
            "Error: product_type '{product_type}' not in product_types (flight|hotel|fit|group_tour)"
        ));
    }
    if let Some(ref b) = blocked {
        if !exists(&conn, "coverage_block_reasons", "code", b).await? {
            return Err(format!("Error: --blocked '{b}' not in coverage_block_reasons"));
        }
    }

    let now = now_db_datetime();
    let proven_int: i64 = if proven { 1 } else { 0 };
    conn.execute(
        "INSERT INTO ota_source_coverage \
            (source_id, product_type, proven, proven_at, method, search_url, blocked_reason_code, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT(source_id, product_type) DO UPDATE SET \
            proven = ?3, \
            proven_at = COALESCE(?4, ota_source_coverage.proven_at), \
            method = COALESCE(?5, ota_source_coverage.method), \
            search_url = COALESCE(?6, ota_source_coverage.search_url), \
            blocked_reason_code = ?7, \
            updated_at = ?8",
        libsql::params![
            source_id.clone(),
            product_type.clone(),
            proven_int,
            proven_at.clone(),
            method.clone(),
            search_url.clone(),
            blocked.clone(),
            now
        ],
    )
    .await
    .map_err(|e| e.to_string())?;

    record_catalog_run(
        &conn,
        "set-ota-coverage",
        &format!("{source_id}/{product_type} proven={proven_int}"),
    )
    .await?;
    println!("ota_source_coverage upserted: {source_id}/{product_type} (proven={proven_int})");
    Ok(())
}

// ---------------------------------------------------------------------------
// set-ota-region <source_id> <product_type> <region_label> <region_code>
// ---------------------------------------------------------------------------
pub async fn run_set_region(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Usage:\n  travel set-ota-region <source_id> <product_type> <region_label> <region_code>");
        return Ok(());
    }
    let pos: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if pos.len() < 4 {
        return Err(
            "Error: set-ota-region requires <source_id> <product_type> <region_label> <region_code>"
                .to_string(),
        );
    }
    let (source_id, product_type, region_label, region_code) =
        (pos[0].to_string(), pos[1].to_string(), pos[2].to_string(), pos[3].to_string());

    let conn = db::connect_write().await?;
    if !exists(&conn, "product_types", "code", &product_type).await? {
        return Err(format!("Error: product_type '{product_type}' not in product_types"));
    }
    conn.execute(
        "INSERT INTO ota_source_region_codes (source_id, product_type, region_label, region_code) \
         VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(source_id, product_type, region_label) DO UPDATE SET region_code = ?4",
        libsql::params![source_id.clone(), product_type.clone(), region_label.clone(), region_code],
    )
    .await
    .map_err(|e| e.to_string())?;

    record_catalog_run(
        &conn,
        "set-ota-region",
        &format!("{source_id}/{product_type}/{region_label}"),
    )
    .await?;
    println!("ota_source_region_codes upserted: {source_id}/{product_type}/{region_label}");
    Ok(())
}

// ---------------------------------------------------------------------------
// set-ota-workflow <source> <product_type> --nav <kind> --url-template <t>
//                  [--capture-url-contains s] [--settle-ms N] [--settle-marker m] [--note ...]
// ---------------------------------------------------------------------------
pub async fn run_set_workflow(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Usage:\n  travel set-ota-workflow <source> <product_type> --nav <kind> \
             --url-template <t> [--capture-url-contains s] [--settle-ms N] [--settle-marker m] [--note ...]"
        );
        return Ok(());
    }
    let pos: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if pos.len() < 2 {
        return Err(
            "Error: set-ota-workflow requires <source> <product_type> --nav <kind> --url-template <t>"
                .to_string(),
        );
    }
    let source_id = pos[0].to_string();
    let product_type = pos[1].to_string();
    let nav = opt_val(args, "--nav")
        .ok_or_else(|| "Error: set-ota-workflow requires --nav <kind>".to_string())?;
    let url_template = opt_val(args, "--url-template")
        .ok_or_else(|| "Error: set-ota-workflow requires --url-template <t>".to_string())?;
    if url_template.trim().is_empty() {
        return Err("Error: --url-template must be non-empty".to_string());
    }
    if nav != "get" && !nav.starts_with("custom:") {
        return Err(format!(
            "Error: --nav must be get or custom:<name> (got {nav})"
        ));
    }
    let capture_url_contains = opt_val(args, "--capture-url-contains");
    let settle_marker = opt_val(args, "--settle-marker");
    let settle_ms_raw = opt_val(args, "--settle-ms");
    let settle_ms: Option<i64> = if let Some(ref raw) = settle_ms_raw {
        Some(
            raw.parse::<i64>()
                .map_err(|e| format!("Error: --settle-ms must be an integer (got {raw}: {e})"))?,
        )
    } else {
        None
    };
    let note = opt_val(args, "--note");

    let conn = db::connect_write().await?;
    if !exists(&conn, "product_types", "code", &product_type).await? {
        return Err(format!(
            "Error: product_type '{product_type}' not in product_types (flight|hotel|fit|group_tour)"
        ));
    }

    let now = now_db_datetime();
    conn.execute(
        "INSERT INTO ota_source_workflow \
            (source_id, product_type, nav_kind, url_template, capture_url_contains, \
             settle_marker, settle_ms, agent_extraction_note, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, COALESCE(?7, 0), ?8, ?9) \
         ON CONFLICT(source_id, product_type) DO UPDATE SET \
            nav_kind = COALESCE(?3, ota_source_workflow.nav_kind), \
            url_template = COALESCE(?4, ota_source_workflow.url_template), \
            capture_url_contains = COALESCE(?5, ota_source_workflow.capture_url_contains), \
            settle_marker = COALESCE(?6, ota_source_workflow.settle_marker), \
            settle_ms = COALESCE(?7, ota_source_workflow.settle_ms), \
            agent_extraction_note = COALESCE(?8, ota_source_workflow.agent_extraction_note), \
            updated_at = ?9",
        libsql::params![
            source_id.clone(),
            product_type.clone(),
            nav.clone(),
            url_template.clone(),
            capture_url_contains.clone(),
            settle_marker.clone(),
            settle_ms,
            note.clone(),
            now,
        ],
    )
    .await
    .map_err(|e| e.to_string())?;

    record_catalog_run(
        &conn,
        "set-ota-workflow",
        &format!("{source_id}/{product_type}"),
    )
    .await?;
    println!("ota_source_workflow upserted: {source_id}/{product_type}");
    Ok(())
}

// ---------------------------------------------------------------------------
// set-ota-url-token <source> <product_type> <placeholder> <input_key> <input_value> <token_value>
// ---------------------------------------------------------------------------
pub async fn run_set_url_token(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Usage:\n  travel set-ota-url-token <source> <product_type> <placeholder> \
             <input_key> <input_value> <token_value>"
        );
        return Ok(());
    }
    let pos: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if pos.len() < 6 {
        return Err(
            "Error: set-ota-url-token requires <source> <product_type> <placeholder> \
             <input_key> <input_value> <token_value>"
                .to_string(),
        );
    }
    let source_id = pos[0].to_string();
    let product_type = pos[1].to_string();
    let placeholder = pos[2].to_string();
    let input_key = pos[3].to_string();
    let input_value = pos[4].to_string();
    let token_value = pos[5].to_string();

    if input_key != "destination" {
        return Err(format!(
            "Error: input_key must be destination (v1 only; got {input_key})"
        ));
    }

    let conn = db::connect_write().await?;
    if !exists(&conn, "product_types", "code", &product_type).await? {
        return Err(format!(
            "Error: product_type '{product_type}' not in product_types (flight|hotel|fit|group_tour)"
        ));
    }

    let now = now_db_datetime();
    conn.execute(
        "INSERT INTO ota_source_url_token \
            (source_id, product_type, placeholder, input_key, input_value, token_value, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT(source_id, product_type, placeholder, input_key, input_value) DO UPDATE SET \
            token_value = ?6, updated_at = ?7",
        libsql::params![
            source_id.clone(),
            product_type.clone(),
            placeholder.clone(),
            input_key.clone(),
            input_value.clone(),
            token_value.clone(),
            now,
        ],
    )
    .await
    .map_err(|e| e.to_string())?;

    record_catalog_run(
        &conn,
        "set-ota-url-token",
        &format!("{source_id}/{product_type}"),
    )
    .await?;
    println!(
        "ota_source_url_token upserted: {source_id}/{product_type}/{placeholder}/{input_key}/{input_value}"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// First value after `flag` (None if absent or no following token).
fn opt_val(args: &[String], flag: &str) -> Option<String> {
    let i = args.iter().position(|a| a == flag)?;
    args.get(i + 1).filter(|v| !v.starts_with("--")).cloned()
}

/// True if `table.col = value` exists (SELECT-validate a reference).
async fn exists(conn: &Connection, table: &str, col: &str, value: &str) -> Result<bool, String> {
    let mut rows = conn
        .query(
            &format!("SELECT 1 FROM {table} WHERE {col} = ?1 LIMIT 1"),
            libsql::params![value.to_string()],
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(rows.next().await.map_err(|e| e.to_string())?.is_some())
}
