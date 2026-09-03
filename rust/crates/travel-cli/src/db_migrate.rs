//! `db migrate` — faithful 1:1 port of scripts/turso-migrate.ts.
//!
//! turso-migrate.ts is the schema source-of-truth: ~116 `CREATE TABLE IF NOT
//! EXISTS` (+ indexes, idempotent ALTER ADD/DROP COLUMN, seed INSERTs, and a set
//! of one-shot de-JSON backfills) run idempotently against Turso. This port
//! produces the IDENTICAL schema by replaying the same statements in the same
//! order, with the same error tolerance.
//!
//! Idempotency contract (mirrors the TS try/catch dances):
//!   - CREATE TABLE / INDEX use `IF NOT EXISTS` (or the TS catches "already
//!     exists").
//!   - ALTER TABLE ADD COLUMN tolerates "duplicate column name".
//!   - ALTER TABLE DROP COLUMN tolerates "no such column" / "has no column".
//!   - The de-JSON backfills are guarded on a legacy `*_json` column still
//!     existing; on the already-migrated live DB they SKIP (exactly like the TS).
//!
//! "Inline DDL" model preserved on purpose: this does NOT execute schema.sql and
//! does NOT switch to turso-util's numbered-migration runner.

use crate::db;

// ---------------------------------------------------------------------------
// Execution helpers — replicate the TS error-tolerance try/catch patterns.
// ---------------------------------------------------------------------------

/// Execute a DDL/DML statement, returning rows-affected. Bubbles the raw error
/// string so callers can pattern-match the message (matching TS `e.message`).
async fn exec(conn: &libsql::Connection, sql: &str) -> Result<u64, String> {
    conn.execute(sql, ()).await.map_err(|e| e.to_string())
}

/// Execute, ignoring ALL errors (used where TS only `console.warn`s and moves on,
/// e.g. DROP TABLE IF EXISTS / seed inserts whose failure is non-fatal).
async fn exec_lenient(conn: &libsql::Connection, sql: &str) {
    if let Err(e) = exec(conn, sql).await {
        eprintln!("⚠️  {e}");
    }
}

/// CREATE-style statement: tolerate "already exists", warn otherwise.
async fn exec_create(conn: &libsql::Connection, sql: &str) {
    if let Err(e) = exec(conn, sql).await {
        if !e.contains("already exists") {
            eprintln!("⚠️  Could not create table: {e}");
        }
    }
}

/// ALTER ... ADD COLUMN: tolerate "duplicate column name" / "already exists".
async fn add_column(conn: &libsql::Connection, sql: &str) {
    if let Err(e) = exec(conn, sql).await {
        let lc = e.to_ascii_lowercase();
        if !(lc.contains("duplicate column") || lc.contains("already exists")) {
            eprintln!("⚠️  Could not add column: {e}");
        }
    }
}

/// ALTER ... DROP COLUMN: tolerate "no such column" / "has no column".
async fn drop_column(conn: &libsql::Connection, table: &str, col: &str) {
    let sql = format!("ALTER TABLE {table} DROP COLUMN {col};");
    if let Err(e) = exec(conn, &sql).await {
        let lc = e.to_ascii_lowercase();
        if !(lc.contains("no such column") || lc.contains("has no column")) {
            eprintln!("⚠️  Could not drop {table}.{col}: {e}");
        }
    }
}

/// Return true if `table` has a column named `col` (PRAGMA table_info guard,
/// used to decide whether a de-JSON backfill still needs to run).
async fn has_column(conn: &libsql::Connection, table: &str, col: &str) -> bool {
    let sql = format!("PRAGMA table_info({table});");
    let Ok(mut rows) = conn.query(&sql, ()).await else {
        return false;
    };
    while let Ok(Some(row)) = rows.next().await {
        // PRAGMA table_info columns: cid, name, type, notnull, dflt_value, pk.
        if let Ok(libsql::Value::Text(name)) = row.get_value(1) {
            if name == col {
                return true;
            }
        }
    }
    false
}

/// Return true if a table with `name` exists.
async fn table_exists(conn: &libsql::Connection, name: &str) -> bool {
    let sql =
        format!("SELECT name FROM sqlite_master WHERE type='table' AND name='{name}'");
    let Ok(mut rows) = conn.query(&sql, ()).await else {
        return false;
    };
    matches!(rows.next().await, Ok(Some(_)))
}

/// Return the CREATE TABLE DDL for `name` from sqlite_master (None if absent).
async fn table_ddl(conn: &libsql::Connection, name: &str) -> Option<String> {
    let sql = format!("SELECT sql FROM sqlite_master WHERE type='table' AND name='{name}'");
    let Ok(mut rows) = conn.query(&sql, ()).await else {
        return None;
    };
    let row = rows.next().await.ok()??;
    row.get::<String>(0).ok()
}

/// Widen `ota_job_params.param_key` CHECK to include `destination` (idempotent table-rebuild).
async fn migrate_ota_job_params_destination(conn: &libsql::Connection) -> Result<(), String> {
    if !table_exists(conn, "ota_job_params").await {
        return Ok(());
    }
    if table_ddl(conn, "ota_job_params")
        .await
        .is_some_and(|ddl| ddl.contains("destination"))
    {
        return Ok(());
    }
    exec(
        conn,
        r#"CREATE TABLE ota_job_params_new (
  job_id TEXT NOT NULL,
  param_key TEXT NOT NULL CHECK(param_key IN ('depart_date','return_date','nights','pax','region_code','region_label','destination')),
  param_value TEXT NOT NULL,
  PRIMARY KEY (job_id, param_key)
)"#,
    )
    .await?;
    exec(
        conn,
        "INSERT INTO ota_job_params_new SELECT * FROM ota_job_params",
    )
    .await?;
    exec(conn, "DROP TABLE ota_job_params").await?;
    exec(
        conn,
        "ALTER TABLE ota_job_params_new RENAME TO ota_job_params",
    )
    .await?;
    Ok(())
}

const OTA_JOB_PARAMS_CHECK: &str = "'depart_date','return_date','nights','pax','region_code','region_label','destination','origin','currency','rooms','hotel'";

/// Widen `ota_job_params.param_key` CHECK to include origin/currency/rooms/hotel (idempotent table-rebuild).
async fn migrate_ota_job_params_common_keys(conn: &libsql::Connection) -> Result<(), String> {
    if !table_exists(conn, "ota_job_params").await {
        return Ok(());
    }
    if table_ddl(conn, "ota_job_params").await.is_some_and(|ddl| {
        ddl.contains("origin")
            && ddl.contains("currency")
            && ddl.contains("rooms")
            && ddl.contains("hotel")
    }) {
        return Ok(());
    }
    let create = format!(
        r#"CREATE TABLE ota_job_params_new (
  job_id TEXT NOT NULL,
  param_key TEXT NOT NULL CHECK(param_key IN ({OTA_JOB_PARAMS_CHECK})),
  param_value TEXT NOT NULL,
  PRIMARY KEY (job_id, param_key)
)"#
    );
    exec(conn, &create).await?;
    exec(
        conn,
        "INSERT INTO ota_job_params_new SELECT * FROM ota_job_params",
    )
    .await?;
    exec(conn, "DROP TABLE ota_job_params").await?;
    exec(
        conn,
        "ALTER TABLE ota_job_params_new RENAME TO ota_job_params",
    )
    .await?;
    Ok(())
}

/// Widen `ota_source_workflow.nav_kind` CHECK to allow `custom:%` (idempotent table-rebuild).
async fn migrate_ota_source_workflow_nav_kind(conn: &libsql::Connection) -> Result<(), String> {
    if !table_exists(conn, "ota_source_workflow").await {
        return Ok(());
    }
    if table_ddl(conn, "ota_source_workflow")
        .await
        .is_some_and(|ddl| ddl.contains("custom:%"))
    {
        return Ok(());
    }
    exec(
        conn,
        r#"CREATE TABLE ota_source_workflow_new (
  source_id TEXT NOT NULL,
  product_type TEXT NOT NULL,
  nav_kind TEXT NOT NULL DEFAULT 'get' CHECK(nav_kind = 'get' OR nav_kind LIKE 'custom:%'),
  url_template TEXT NOT NULL,
  capture_url_contains TEXT,
  settle_marker TEXT,
  settle_ms INTEGER NOT NULL DEFAULT 0,
  agent_extraction_note TEXT,
  updated_at TEXT NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY (source_id, product_type)
)"#,
    )
    .await?;
    exec(
        conn,
        "INSERT INTO ota_source_workflow_new SELECT * FROM ota_source_workflow",
    )
    .await?;
    exec(conn, "DROP TABLE ota_source_workflow").await?;
    exec(
        conn,
        "ALTER TABLE ota_source_workflow_new RENAME TO ota_source_workflow",
    )
    .await?;
    Ok(())
}

/// Rename `ota_source_url_token` → `ota_source_url_param` (clearer column naming, spec 2026-07-01):
/// placeholder→url_param_name, input_key→input_name, token_value→url_value. Rebuild-and-copy so any
/// live-only rows (e.g. a CLI-registered travel4u) are preserved via INSERT ... SELECT. Idempotent:
/// the new table is created first (above); this only migrates + drops the OLD table if it still exists.
async fn migrate_ota_source_url_token_to_url_param(conn: &libsql::Connection) -> Result<(), String> {
    if !table_exists(conn, "ota_source_url_token").await {
        return Ok(());
    }
    exec(
        conn,
        "INSERT OR IGNORE INTO ota_source_url_param \
           (source_id, product_type, url_param_name, input_name, input_value, url_value, updated_at) \
         SELECT source_id, product_type, placeholder, input_key, input_value, token_value, updated_at \
         FROM ota_source_url_token",
    )
    .await?;
    exec(conn, "DROP TABLE ota_source_url_token").await?;
    Ok(())
}

/// SQL string-literal escaper (single quotes doubled), matching the TS `esc`.
fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

// ---------------------------------------------------------------------------
// Entry point.
// ---------------------------------------------------------------------------

pub async fn run(args: &[String]) -> Result<(), String> {
    let _ = args;
    let conn = db::connect_write().await?;

    println!("Running Turso schema migrations...");

    // 1. events.external_id column.
    add_column(&conn, "ALTER TABLE events ADD COLUMN external_id TEXT;").await;
    // 2. unique index on events.external_id (TS uses a non-IF-NOT-EXISTS CREATE
    //    UNIQUE INDEX and catches "already exists").
    if let Err(e) = exec(
        &conn,
        "CREATE UNIQUE INDEX idx_events_external_id ON events(external_id);",
    )
    .await
    {
        if !e.contains("already exists") {
            eprintln!("⚠️  Could not create index: {e}");
        }
    }
    // 3. offers.source_file column.
    add_column(&conn, "ALTER TABLE offers ADD COLUMN source_file TEXT;").await;
    // 4. dedup index on offers(id, scraped_at).
    exec_lenient(
        &conn,
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_offers_dedup ON offers(id, scraped_at);",
    )
    .await;

    // 5. bookings table.
    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS bookings (
  destination TEXT NOT NULL,
  offer_id TEXT NOT NULL,
  selected_date TEXT NOT NULL,
  price_per_person INTEGER,
  price_total INTEGER,
  currency TEXT DEFAULT 'TWD',
  status TEXT CHECK(status IN ('selected', 'booked', 'confirmed')),
  source_id TEXT,
  hotel_name TEXT,
  airline TEXT,
  flight_out TEXT,
  flight_return TEXT,
  selected_at DATETIME,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (destination, offer_id)
);"#,
    )
    .await;

    // 6. Drop obsolete plan_snapshots.
    exec_lenient(&conn, "DROP TABLE IF EXISTS plan_snapshots;").await;

    // 7. bookings_current.
    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS bookings_current (
  booking_key TEXT PRIMARY KEY,
  trip_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  category TEXT NOT NULL CHECK(category IN ('package','transfer','activity')),
  subtype TEXT,
  title TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('pending','planned','booked','confirmed','waitlist','skipped','cancelled')),
  reference TEXT,
  book_by TEXT,
  booked_at TEXT,
  source_id TEXT,
  offer_id TEXT,
  selected_date TEXT,
  price_amount INTEGER,
  price_currency TEXT DEFAULT 'TWD',
  origin_path TEXT,
  payload_text TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);"#,
    )
    .await;
    exec_lenient(
        &conn,
        "CREATE INDEX IF NOT EXISTS idx_bc_dest ON bookings_current(destination, category);",
    )
    .await;
    exec_lenient(
        &conn,
        "CREATE INDEX IF NOT EXISTS idx_bc_status ON bookings_current(status);",
    )
    .await;
    exec_lenient(
        &conn,
        "CREATE INDEX IF NOT EXISTS idx_bc_offer ON bookings_current(offer_id);",
    )
    .await;

    // 8. bookings_events.
    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS bookings_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  booking_key TEXT NOT NULL,
  event_type TEXT NOT NULL,
  previous_status TEXT,
  new_status TEXT,
  reference TEXT,
  book_by TEXT,
  amount INTEGER,
  currency TEXT,
  event_data_text TEXT,
  event_at DATETIME DEFAULT CURRENT_TIMESTAMP
);"#,
    )
    .await;
    exec_lenient(
        &conn,
        "CREATE INDEX IF NOT EXISTS idx_be_key ON bookings_events(booking_key, event_at);",
    )
    .await;

    // 9. plans table — skip if plans_current still exists (step 13 renames it).
    if table_exists(&conn, "plans_current").await {
        println!("ℹ️  plans_current exists — step 13 will rename it to plans.");
    } else {
        exec_create(
            &conn,
            r#"CREATE TABLE IF NOT EXISTS plans (
  plan_id TEXT PRIMARY KEY,
  schema_version TEXT NOT NULL,
  plan_json TEXT NOT NULL,
  state_json TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);"#,
        )
        .await;
    }

    // 10. Normalized itinerary tables (Phase 1).
    for sql in ITINERARY_TABLES {
        exec_create(&conn, sql).await;
    }
    exec_lenient(
        &conn,
        "CREATE INDEX IF NOT EXISTS idx_activities_session ON activities(plan_id, destination, day_number, session_type, sort_order)",
    )
    .await;
    exec_lenient(
        &conn,
        "CREATE INDEX IF NOT EXISTS idx_activities_booking ON activities(plan_id, booking_status)",
    )
    .await;

    // 11. version column on the plans/plans_current table (whichever exists).
    let plans_table = if table_exists(&conn, "plans_current").await {
        "plans_current"
    } else {
        "plans"
    };
    add_column(
        &conn,
        &format!("ALTER TABLE {plans_table} ADD COLUMN version INTEGER NOT NULL DEFAULT 0;"),
    )
    .await;

    // 12. operation_runs.
    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS operation_runs (
  run_id TEXT PRIMARY KEY,
  plan_id TEXT NOT NULL,
  command_type TEXT NOT NULL,
  command_summary TEXT,
  status TEXT NOT NULL DEFAULT 'started'
    CHECK(status IN ('started', 'completed', 'failed')),
  version_before INTEGER NOT NULL,
  version_after INTEGER,
  error_message TEXT,
  started_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
  completed_at DATETIME,
  idempotency_key TEXT
);"#,
    )
    .await;
    exec_lenient(
        &conn,
        "CREATE INDEX IF NOT EXISTS idx_operation_runs_plan ON operation_runs(plan_id, started_at DESC);",
    )
    .await;
    exec_lenient(
        &conn,
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_operation_runs_idempotency ON operation_runs(plan_id, idempotency_key);",
    )
    .await;

    // 12b. plan_map_snapshots — records when the dashboard's static map PNGs
    //      were last snapshotted (by scripts/snapshot-maps.sh via chromeport→R2)
    //      so the `check-maps-fresh` lint can flag maps that have gone stale
    //      relative to the latest itinerary edit. Side table keyed by plan_id.
    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS plan_map_snapshots (
  plan_id TEXT NOT NULL PRIMARY KEY,
  snapshotted_at TEXT NOT NULL
);"#,
    )
    .await;

    // 12c. map_artifacts — manifest of dashboard map PNGs uploaded to R2 by
    //      scripts/snapshot-maps.sh. The CLI lint reads this (no R2 client) to
    //      flag MISSING/EMPTY keys per plan.
    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS map_artifacts (
  plan_id TEXT NOT NULL,
  map_key TEXT NOT NULL,
  byte_size INTEGER,
  sha256 TEXT,
  status TEXT NOT NULL,
  skip_reason TEXT,
  generated_at TEXT NOT NULL,
  PRIMARY KEY (plan_id, map_key)
);"#,
    )
    .await;

    // 12d. route_place_geocodes — keyless-geocoding cache (Nominatim/OSM) for
    //      day_route_segments place names (hotel/airport/restaurant/mall/etc. that
    //      are NOT sightseeing destination_pois). snapshot-maps.sh resolves each
    //      route place to lat/lon via this cache (write-through), so a re-run is
    //      free and we respect Nominatim's ≤1 req/s policy. `query_key` is the
    //      normalized lookup key; `confidence`/`review` gate plotting low-quality
    //      matches; `failure_reason` records a no-result so we don't re-hit it.
    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS route_place_geocodes (
  query_key TEXT NOT NULL,
  raw_place TEXT NOT NULL,
  lat REAL,
  lon REAL,
  display_name TEXT,
  osm_id TEXT,
  osm_type TEXT,
  provider TEXT NOT NULL,
  confidence TEXT,
  review INTEGER NOT NULL DEFAULT 0,
  failure_reason TEXT,
  fetched_at TEXT NOT NULL,
  PRIMARY KEY (query_key)
);"#,
    )
    .await;

    // 12e. route_road_legs / route_road_leg_points — keyless road-geometry cache (OSRM
    //      public demo) for snapshot-maps.sh Tier 2. One LEG = one ordered pair of stop
    //      coords (a per-day route is N-1 legs). The header row records provider/profile/
    //      status (ok|error) + fetched_at so a re-run makes ZERO OSRM calls and
    //      failures aren't re-hit; the child rows store the road polyline as NORMALIZED
    //      point rows (NO JSON blob — repo rule). leg_key = canonicalized
    //      "from_lat,from_lon>to_lat,to_lon|provider|profile" at fixed precision.
    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS route_road_legs (
  leg_key TEXT NOT NULL,
  from_lat REAL NOT NULL,
  from_lon REAL NOT NULL,
  to_lat REAL NOT NULL,
  to_lon REAL NOT NULL,
  provider TEXT NOT NULL,
  profile TEXT NOT NULL,
  status TEXT NOT NULL,
  point_count INTEGER NOT NULL DEFAULT 0,
  distance_m REAL,
  failure_reason TEXT,
  fetched_at TEXT NOT NULL,
  PRIMARY KEY (leg_key)
);"#,
    )
    .await;
    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS route_road_leg_points (
  leg_key TEXT NOT NULL,
  point_order INTEGER NOT NULL,
  lat REAL NOT NULL,
  lon REAL NOT NULL,
  PRIMARY KEY (leg_key, point_order)
);"#,
    )
    .await;

    // 13. Rename plans_current → plans.
    if table_exists(&conn, "plans_current").await {
        exec_lenient(&conn, "ALTER TABLE plans_current RENAME TO plans").await;
    }

    // 13b. Soft-delete flag on plans. NULL = live; a timestamp = soft-deleted
    //      (set by `mark-plan-deleted`, wiped in batch by `db cleanup-deleted`).
    //      Idempotent ADD COLUMN — tolerated "duplicate column" on re-run.
    add_column(&conn, "ALTER TABLE plans ADD COLUMN deleted_at TEXT;").await;

    // 14. feels_like columns on days.
    add_column(&conn, "ALTER TABLE days ADD COLUMN feels_like_low_c REAL;").await;
    add_column(&conn, "ALTER TABLE days ADD COLUMN feels_like_high_c REAL;").await;

    // 14b. lat/lon on destination_pois (map pins for the dashboard; sourced via geocoder).
    add_column(&conn, "ALTER TABLE destination_pois ADD COLUMN lat REAL;").await;
    add_column(&conn, "ALTER TABLE destination_pois ADD COLUMN lon REAL;").await;

    // 14c. voucher_url on hotels (dashboard hotel-section PDF link → /voucher/* R2 route).
    add_column(&conn, "ALTER TABLE hotels ADD COLUMN voucher_url TEXT;").await;

    // 14d. poi_id FK on activities — a durable activity→POI link so the dashboard
    //      attaches ticket price + map pin by id, not fragile title equality
    //      (set via `travel set-activity-poi`; title-match stays as a fallback).
    //      Idempotent ADD COLUMN — tolerated "duplicate column" on re-run.
    add_column(&conn, "ALTER TABLE activities ADD COLUMN poi_id TEXT;").await;

    // 14e. source provenance on itinerary content tables (confirmed vs ai_recommended).
    add_column(
        &conn,
        "ALTER TABLE activities ADD COLUMN source TEXT NOT NULL DEFAULT 'confirmed';",
    )
    .await;
    add_column(
        &conn,
        "ALTER TABLE session_meals ADD COLUMN source TEXT NOT NULL DEFAULT 'confirmed';",
    )
    .await;
    add_column(
        &conn,
        "ALTER TABLE day_route_segments ADD COLUMN source TEXT NOT NULL DEFAULT 'confirmed';",
    )
    .await;

    // 15. flight_legs (normalized — replaces JSON blobs in flights).
    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS flight_legs (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  direction TEXT NOT NULL CHECK(direction IN ('outbound', 'return')),
  leg_order INTEGER NOT NULL DEFAULT 0,
  flight_number TEXT,
  airline TEXT,
  airline_code TEXT,
  departure_airport TEXT,
  departure_code TEXT,
  departure_terminal TEXT,
  departure_time TEXT,
  arrival_airport TEXT,
  arrival_code TEXT,
  arrival_terminal TEXT,
  arrival_time TEXT,
  flight_date TEXT,
  populated_from TEXT,
  booked_date TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, direction, leg_order)
);"#,
    )
    .await;
    // 15b. flights → flight_legs migration: the legacy `flights` table is dropped
    //      in Batch F; on the live (already-migrated) DB it no longer exists, so
    //      this is a no-op. See migrate_flights_to_legs().
    migrate_flights_to_legs(&conn).await;

    // Phase 1: full normalization tables (v5.0.0).
    for sql in PHASE1_TABLES {
        exec_create(&conn, sql).await;
    }

    // Phase 1b. Grant-token management columns for the dashboard. Existing
    // plan_share_tokens rows default to active; dashboard mutation routes write
    // created_by/deactivated_* for owner-visible history.
    add_column(
        &conn,
        "ALTER TABLE plan_share_tokens ADD COLUMN status TEXT NOT NULL DEFAULT 'active';",
    )
    .await;
    add_column(&conn, "ALTER TABLE plan_share_tokens ADD COLUMN created_by TEXT;").await;
    add_column(&conn, "ALTER TABLE plan_share_tokens ADD COLUMN deactivated_at TEXT;").await;
    add_column(&conn, "ALTER TABLE plan_share_tokens ADD COLUMN deactivated_by TEXT;").await;
    exec_lenient(
        &conn,
        "CREATE INDEX IF NOT EXISTS idx_plan_share_tokens_plan_status_created ON plan_share_tokens(plan_id, status, created_at DESC);",
    )
    .await;

    // selected_* scalar columns on airport_transfers.
    for col in [
        "selected_title TEXT",
        "selected_route TEXT",
        "selected_duration_min INTEGER",
        "selected_price_yen INTEGER",
        "selected_schedule TEXT",
        "selected_booking_url TEXT",
        "selected_notes TEXT",
    ] {
        add_column(
            &conn,
            &format!("ALTER TABLE airport_transfers ADD COLUMN {col};"),
        )
        .await;
    }

    // Phase 6: drop blob columns from plans.
    drop_column(&conn, "plans", "plan_json").await;
    drop_column(&conn, "plans", "state_json").await;

    // ZH content ALTERs.
    for (table, col) in [
        ("days", "theme_zh TEXT"),
        ("timesofday", "focus_zh TEXT"),
        ("timesofday", "transit_notes_zh TEXT"),
        ("hotels", "name_zh TEXT"),
        ("itinerary_metadata", "transit_summary_zh TEXT"),
        ("plan_destinations", "home_address TEXT"),
    ] {
        add_column(&conn, &format!("ALTER TABLE {table} ADD COLUMN {col};")).await;
    }

    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS day_route_segments (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  day_number INTEGER NOT NULL, sort_order INTEGER NOT NULL,
  from_place TEXT NOT NULL, to_place TEXT NOT NULL,
  mode TEXT NOT NULL,
  source TEXT NOT NULL DEFAULT 'confirmed' CHECK(source IN ('confirmed', 'ai_recommended')),
  PRIMARY KEY (plan_id, destination, day_number, sort_order)
)"#,
    )
    .await;
    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS day_landmarks (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  day_number INTEGER NOT NULL, sort_order INTEGER NOT NULL,
  landmark TEXT NOT NULL,
  PRIMARY KEY (plan_id, destination, day_number, sort_order)
)"#,
    )
    .await;

    // display_name on destination_cities.
    add_column(
        &conn,
        "ALTER TABLE destination_cities ADD COLUMN display_name TEXT;",
    )
    .await;
    // offer_count on plan_offer_provenance.
    add_column(
        &conn,
        "ALTER TABLE plan_offer_provenance ADD COLUMN offer_count INTEGER;",
    )
    .await;

    // Global config tables.
    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS destination_config (
  slug TEXT PRIMARY KEY,
  display_name TEXT NOT NULL,
  ref_id TEXT,
  ref_path TEXT,
  timezone TEXT NOT NULL DEFAULT 'Asia/Tokyo',
  currency TEXT NOT NULL DEFAULT 'JPY',
  markets_json TEXT,
  primary_airports_json TEXT,
  language TEXT DEFAULT 'ja',
  origin TEXT DEFAULT 'taiwan',
  lat REAL,
  lon REAL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
)"#,
    )
    .await;
    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS origin_config (
  slug TEXT PRIMARY KEY,
  country_code TEXT,
  currency TEXT,
  timezone TEXT,
  holiday_calendar TEXT,
  primary_airports_json TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
)"#,
    )
    .await;
    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS global_config (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
)"#,
    )
    .await;

    // Child tables for destination/origin airports + markets.
    exec_lenient(
        &conn,
        "CREATE TABLE IF NOT EXISTS destination_airports (slug TEXT, airport TEXT, sort_order INTEGER, PRIMARY KEY (slug, sort_order));",
    )
    .await;
    exec_lenient(
        &conn,
        "CREATE TABLE IF NOT EXISTS destination_markets (slug TEXT, market TEXT, sort_order INTEGER, PRIMARY KEY (slug, sort_order));",
    )
    .await;
    exec_lenient(
        &conn,
        "CREATE TABLE IF NOT EXISTS origin_airports (slug TEXT, airport TEXT, sort_order INTEGER, PRIMARY KEY (slug, sort_order));",
    )
    .await;

    backfill_destination_config(&conn).await;
    exec_lenient(
        &conn,
        "UPDATE destination_config SET ref_path = '' WHERE slug = 'kyoto_2026' AND ref_path = 'src/skills/travel-shared/references/destinations/kyoto.json';",
    )
    .await;
    backfill_origin_config(&conn).await;
    backfill_global_config(&conn).await;

    // OTA sources table + child tables + seed rows.
    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS ota_sources (
  source_id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  type_json TEXT,
  status TEXT DEFAULT 'active',
  scraper_script TEXT,
  regions_json TEXT,
  url_template TEXT,
  notes TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
)"#,
    )
    .await;
    exec_lenient(
        &conn,
        "CREATE TABLE IF NOT EXISTS ota_source_types (source_id TEXT, type TEXT, PRIMARY KEY (source_id, type));",
    )
    .await;
    exec_lenient(
        &conn,
        "CREATE TABLE IF NOT EXISTS ota_source_regions (source_id TEXT, region TEXT, PRIMARY KEY (source_id, region));",
    )
    .await;
    // Normalized OTA provider catalog (DB-centric provider architecture, spec
    // 2026-06-29). product_types is the ONE canonical type list; coverage + the
    // re-keyed parser_rules both reference it. No FK clauses (the repo doesn't
    // enable PRAGMA foreign_keys; validity is enforced by the write CLI + validate).
    exec_lenient(
        &conn,
        "CREATE TABLE IF NOT EXISTS product_types (code TEXT PRIMARY KEY, description TEXT);",
    )
    .await;
    exec_lenient(
        &conn,
        "CREATE TABLE IF NOT EXISTS coverage_block_reasons (code TEXT PRIMARY KEY, description TEXT);",
    )
    .await;
    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS ota_source_coverage (
  source_id TEXT NOT NULL,
  product_type TEXT NOT NULL,
  proven INTEGER NOT NULL DEFAULT 0 CHECK(proven IN (0,1)),
  proven_at TEXT,
  method TEXT CHECK(method IS NULL OR method IN ('agent_parse','regex')),
  search_url TEXT,
  blocked_reason_code TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (source_id, product_type)
)"#,
    )
    .await;
    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS ota_source_region_codes (
  source_id TEXT NOT NULL,
  product_type TEXT NOT NULL,
  region_label TEXT NOT NULL,
  region_code TEXT NOT NULL,
  PRIMARY KEY (source_id, product_type, region_label)
)"#,
    )
    .await;
    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS catalog_runs (
  run_id TEXT PRIMARY KEY,
  command_type TEXT NOT NULL,
  command_summary TEXT,
  status TEXT NOT NULL,
  changed_at TEXT NOT NULL
)"#,
    )
    .await;
    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS ota_notes_migration_audit (
  source_id TEXT,
  raw_note TEXT,
  checksum TEXT,
  normalized_at TEXT,
  disposition TEXT CHECK(disposition IN ('normalized','discarded_recipe'))
)"#,
    )
    .await;
    seed_ota_sources(&conn).await;
    seed_ota_catalog(&conn).await;
    seed_ota_coverage(&conn).await;
    backfill_ota_notes_audit(&conn).await;
    // NOTE: the `parser_rules` table (regex/custom-parser path) is RETIRED — extraction is
    // agent-first (the agent reads a capture's raw_text and feeds offers to `ota write-offers`).
    // travel-cli no longer creates/reads parser_rules; any existing live table is left orphaned.

    // OTA execution job lifecycle (rust-first OTA architecture, spec 2026-06-29).
    // No FK clauses (repo doesn't enable PRAGMA foreign_keys; validity is CLI + validate).
    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS ota_jobs (
  job_id TEXT PRIMARY KEY,
  source_id TEXT NOT NULL,
  product_type TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'queued' CHECK(status IN ('queued','claimed','running','succeeded','failed','blocked')),
  claimed_by TEXT,
  claimed_at TEXT,
  claim_token TEXT,
  lease_expires_at TEXT,
  heartbeat_at TEXT,
  attempts INTEGER NOT NULL DEFAULT 0,
  max_attempts INTEGER NOT NULL DEFAULT 3,
  next_retry_at TEXT,
  blocked_reason_code TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  CHECK(status!='blocked' OR blocked_reason_code IS NOT NULL)
)"#,
    )
    .await;
    exec_create(
        &conn,
        &format!(
            r#"CREATE TABLE IF NOT EXISTS ota_job_params (
  job_id TEXT NOT NULL,
  param_key TEXT NOT NULL CHECK(param_key IN ({OTA_JOB_PARAMS_CHECK})),
  param_value TEXT NOT NULL,
  PRIMARY KEY (job_id, param_key)
)"#
        ),
    )
    .await;
    migrate_ota_job_params_destination(&conn).await?;
    migrate_ota_job_params_common_keys(&conn).await?;
    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS ota_attempts (
  attempt_id TEXT PRIMARY KEY,
  job_id TEXT NOT NULL,
  attempt_no INTEGER NOT NULL,
  claim_token TEXT NOT NULL,
  outcome TEXT NOT NULL CHECK(outcome IN ('succeeded','failed','blocked')),
  capture_id TEXT,
  candidate_count INTEGER NOT NULL DEFAULT 0,
  inserted_count INTEGER NOT NULL DEFAULT 0,
  deduped_count INTEGER NOT NULL DEFAULT 0,
  error_detail TEXT,
  started_at TEXT NOT NULL,
  finished_at TEXT,
  UNIQUE(job_id, attempt_no)
)"#,
    )
    .await;
    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS ota_observations (
  observation_id TEXT PRIMARY KEY,
  source_id TEXT NOT NULL,
  product_type TEXT,
  job_id TEXT,
  attempt_id TEXT,
  observation_type TEXT NOT NULL CHECK(observation_type IN ('block','captcha','login_wall','render_error','rate_limit','parse_warning','freshness','empty_result')),
  block_reason_code TEXT,
  severity TEXT NOT NULL DEFAULT 'warn' CHECK(severity IN ('info','warn','error')),
  http_status INTEGER,
  field_name TEXT,
  selector TEXT,
  expected_value TEXT,
  observed_value TEXT,
  duration_ms INTEGER,
  freshness_reference_at TEXT,
  detail TEXT,
  observed_at TEXT NOT NULL
)"#,
    )
    .await;
    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS ota_source_workflow (
  source_id TEXT NOT NULL,
  product_type TEXT NOT NULL,
  nav_kind TEXT NOT NULL DEFAULT 'get' CHECK(nav_kind = 'get' OR nav_kind LIKE 'custom:%'),
  url_template TEXT NOT NULL,
  capture_url_contains TEXT,
  settle_marker TEXT,
  settle_ms INTEGER NOT NULL DEFAULT 0,
  agent_extraction_note TEXT,
  updated_at TEXT NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY (source_id, product_type)
)"#,
    )
    .await;
    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS ota_source_url_param (
  source_id TEXT NOT NULL,
  product_type TEXT NOT NULL,
  url_param_name TEXT NOT NULL,
  input_name TEXT NOT NULL,
  input_value TEXT NOT NULL,
  url_value TEXT NOT NULL,
  updated_at TEXT NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY (source_id, product_type, url_param_name, input_name, input_value)
)"#,
    )
    .await;
    migrate_ota_source_url_token_to_url_param(&conn).await?;
    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS product_type_inputs (
  product_type TEXT NOT NULL,
  input_name TEXT NOT NULL,
  input_class TEXT NOT NULL CHECK(input_class IN ('common','token_key')),
  required INTEGER NOT NULL DEFAULT 1 CHECK(required IN (0,1)),
  default_source TEXT CHECK(default_source IS NULL OR default_source IN ('caller','db','code')),
  sort_order INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (product_type, input_name)
)"#,
    )
    .await;
    migrate_ota_source_workflow_nav_kind(&conn).await?;
    seed_ota_workflow(&conn).await;
    seed_ota_url_param(&conn).await;
    seed_product_type_inputs(&conn).await;

    // OTA offer provenance (rust-first OTA architecture, spec 2026-06-29).
    add_column(&conn, "ALTER TABLE offers ADD COLUMN capture_id TEXT;").await;
    add_column(&conn, "ALTER TABLE offers ADD COLUMN produced_by_job_id TEXT;").await;
    add_column(&conn, "ALTER TABLE offers ADD COLUMN produced_by_attempt_id TEXT;").await;
    add_column(
        &conn,
        "ALTER TABLE offers ADD COLUMN parser_method TEXT CHECK(parser_method IS NULL OR parser_method IN ('agent_parse','regex'));",
    )
    .await;
    add_column(&conn, "ALTER TABLE offers ADD COLUMN capture_checksum TEXT;").await;
    add_column(&conn, "ALTER TABLE offers ADD COLUMN parser_rule_checksum TEXT;").await;
    add_column(&conn, "ALTER TABLE offers ADD COLUMN normalizer_version TEXT;").await;

    // Content-hash re-ingest dedup (plan docs/plans/2026-08-29-offers-dedup-a-prime.md):
    // offer_key = stable pre-disambiguation identity, dedup_key = sha256 content fingerprint
    // (UNIQUE — all-NULL rows are distinct in SQLite, so creating it before backfill is safe),
    // last_seen_at = bumped whenever content is re-observed.
    add_column(&conn, "ALTER TABLE offers ADD COLUMN offer_key TEXT;").await;
    add_column(&conn, "ALTER TABLE offers ADD COLUMN dedup_key TEXT;").await;
    add_column(&conn, "ALTER TABLE offers ADD COLUMN last_seen_at TEXT;").await;
    exec_lenient(
        &conn,
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_offers_dedup_key ON offers(dedup_key);",
    )
    .await;
    exec_lenient(
        &conn,
        "CREATE INDEX IF NOT EXISTS idx_offers_offer_key ON offers(offer_key, scraped_at);",
    )
    .await;
    exec_lenient(
        &conn,
        "CREATE INDEX IF NOT EXISTS idx_offers_last_seen ON offers(last_seen_at);",
    )
    .await;
    backfill_offers_dedup_keys(&conn).await;

    // Shaping Stage research tables.
    for sql in SHAPING_RESEARCH_TABLES {
        exec_lenient(&conn, sql).await;
    }
    exec_lenient(
        &conn,
        "CREATE INDEX IF NOT EXISTS idx_s0_cand_run ON shaping_candidates(run_id, rank);",
    )
    .await;

    exec_lenient(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS shaping_rules (
  shaping_id INTEGER PRIMARY KEY AUTOINCREMENT,
  run_id TEXT NOT NULL,
  aspect TEXT NOT NULL,
  role TEXT NOT NULL,
  kind TEXT NOT NULL,
  value_text TEXT,
  value_date TEXT,
  value_integer INTEGER,
  notes TEXT,
  created_at TEXT NOT NULL
);"#,
    )
    .await;
    exec_lenient(
        &conn,
        "CREATE INDEX IF NOT EXISTS idx_s0_shaping_run ON shaping_rules(run_id, aspect, role);",
    )
    .await;
    exec_lenient(
        &conn,
        r#"CREATE UNIQUE INDEX IF NOT EXISTS uq_s0_shaping_value
  ON shaping_rules(
    run_id, aspect, role, kind,
    COALESCE(value_text, ''),
    COALESCE(value_date, ''),
    COALESCE(value_integer, 0)
  );"#,
    )
    .await;

    exec_lenient(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS shaping_tour_group_offers (
  run_id TEXT NOT NULL,
  offer_id TEXT NOT NULL,
  source_id TEXT NOT NULL,
  dest_region TEXT NOT NULL,
  depart_date TEXT NOT NULL,
  return_date TEXT NOT NULL,
  nights INTEGER NOT NULL,
  price_per_person_twd INTEGER NOT NULL,
  title TEXT NOT NULL,
  url TEXT NOT NULL,
  scraped_at TEXT NOT NULL,
  hotel_name TEXT,
  hotel_star_rating INTEGER,
  meals_included_count INTEGER,
  departure_status TEXT,
  seats_available INTEGER,
  min_group_size INTEGER,
  group_size_cap INTEGER,
  raw_text TEXT,
  parse_warnings_text TEXT,
  PRIMARY KEY (run_id, offer_id)
);"#,
    )
    .await;
    exec_lenient(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS shaping_tour_group_scrape_attempts (
  run_id TEXT NOT NULL,
  source_id TEXT NOT NULL,
  dest_region TEXT NOT NULL,
  nights INTEGER NOT NULL,
  status TEXT NOT NULL,
  offer_count INTEGER,
  parsed_count INTEGER,
  skipped_count INTEGER,
  error TEXT,
  attempted_at TEXT,
  PRIMARY KEY (run_id, source_id, dest_region, nights)
);"#,
    )
    .await;
    exec_lenient(
        &conn,
        "CREATE INDEX IF NOT EXISTS idx_s0_tg_offers_lookup ON shaping_tour_group_offers(run_id, dest_region, nights, price_per_person_twd);",
    )
    .await;

    exec_lenient(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS shaping_research_artifacts (
  artifact_id TEXT PRIMARY KEY,
  run_id TEXT,
  destination_slug TEXT,
  artifact_kind TEXT,
  original_filename TEXT,
  payload_text TEXT,
  raw_text TEXT,
  observed_by TEXT,
  observed_at TEXT,
  imported_at TEXT
);"#,
    )
    .await;
    exec_lenient(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS shaping_selected_offers (
  selection_id TEXT PRIMARY KEY,
  run_id TEXT,
  destination_slug TEXT,
  source_id TEXT,
  source_offer_id TEXT,
  selected_depart_date TEXT,
  selected_return_date TEXT,
  nights INTEGER,
  price_per_person_twd INTEGER,
  price_total_twd INTEGER,
  hotel_name TEXT,
  observed_by TEXT,
  observed_at TEXT,
  selected_by TEXT,
  selected_at TEXT,
  provenance_text TEXT,
  raw_text TEXT,
  imported_at TEXT
);"#,
    )
    .await;

    // product_kind on shaping_tour_group_offers.
    add_column(
        &conn,
        "ALTER TABLE shaping_tour_group_offers ADD COLUMN product_kind TEXT NOT NULL DEFAULT 'group_tour';",
    )
    .await;

    // package_subtype on plan_offers + default backfill.
    add_column(&conn, "ALTER TABLE plan_offers ADD COLUMN package_subtype TEXT;").await;
    exec_lenient(
        &conn,
        "UPDATE plan_offers SET package_subtype = 'fit' WHERE package_subtype IS NULL;",
    )
    .await;

    exec_lenient(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS plan_offer_group_meta (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  offer_id TEXT NOT NULL,
  meals_included_count INTEGER,
  departure_status TEXT,
  seats_available INTEGER,
  min_group_size INTEGER,
  group_size_cap INTEGER,
  source_offer_run_id TEXT,
  source_offer_id TEXT,
  PRIMARY KEY (plan_id, destination, offer_id),
  FOREIGN KEY (plan_id, destination, offer_id) REFERENCES plan_offers(plan_id, destination, id)
);"#,
    )
    .await;

    exec_lenient(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS offers (
  id TEXT,
  source_file TEXT,
  source_id TEXT,
  type TEXT,
  name TEXT,
  price_per_person INTEGER,
  currency TEXT,
  region TEXT,
  destination TEXT,
  departure_date TEXT,
  return_date TEXT,
  nights INTEGER,
  availability TEXT,
  hotel_name TEXT,
  airline TEXT,
  raw_data TEXT,
  scraped_at TEXT,
  PRIMARY KEY (id, scraped_at)
);"#,
    )
    .await;

    exec_lenient(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS hotel_areas (
  region TEXT,
  area_type TEXT,
  source_url TEXT,
  fetched_at TEXT,
  confidence TEXT,
  PRIMARY KEY (region, area_type)
);"#,
    )
    .await;
    exec_lenient(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS transport_routes (
  region TEXT,
  route_key TEXT,
  from_hub TEXT,
  to_hub TEXT,
  time_min INTEGER,
  cost_jpy INTEGER,
  method TEXT,
  source_url TEXT,
  fetched_at TEXT,
  confidence TEXT,
  PRIMARY KEY (region, route_key)
);"#,
    )
    .await;
    exec_lenient(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS transport_hubs (
  region TEXT,
  hub_id TEXT,
  hub_type TEXT,
  area TEXT,
  source_url TEXT,
  fetched_at TEXT,
  confidence TEXT,
  PRIMARY KEY (region, hub_id)
);"#,
    )
    .await;
    exec_lenient(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS destination_references (
  slug TEXT PRIMARY KEY,
  ref_id TEXT,
  payload_json TEXT,
  source_url TEXT,
  fetched_at TEXT,
  confidence TEXT
);"#,
    )
    .await;

    // Holidays table (+ idempotent column adds + year backfill + index).
    exec_lenient(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS holidays (
  country TEXT,
  year INTEGER,
  date TEXT,
  name TEXT,
  day_of_week TEXT,
  is_holiday INTEGER,
  source_url TEXT,
  source_label TEXT,
  fetched_at TEXT,
  confidence TEXT,
  PRIMARY KEY (country, date)
);"#,
    )
    .await;
    add_column(&conn, "ALTER TABLE holidays ADD COLUMN year INTEGER;").await;
    add_column(&conn, "ALTER TABLE holidays ADD COLUMN confidence TEXT;").await;
    exec_lenient(
        &conn,
        "UPDATE holidays SET year = CAST(substr(date, 1, 4) AS INTEGER) WHERE year IS NULL AND date IS NOT NULL;",
    )
    .await;
    exec_lenient(
        &conn,
        "CREATE INDEX IF NOT EXISTS idx_holidays_country_date ON holidays(country, date);",
    )
    .await;

    // OTA domain-knowledge reference tables.
    exec_lenient(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS airlines (
  code TEXT PRIMARY KEY,
  name TEXT,
  type TEXT,
  hand_baggage_kg INTEGER,
  checked_bag_included INTEGER,
  checked_bag_kg INTEGER,
  checked_bag_cost_twd INTEGER,
  checked_bag_cost_jpy INTEGER,
  classification_baggage_note TEXT,
  classification_meal TEXT,
  source_url TEXT,
  fetched_at TEXT,
  confidence TEXT
);"#,
    )
    .await;
    exec_lenient(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS booking_types (
  slug TEXT PRIMARY KEY,
  name_zh TEXT,
  description TEXT,
  source_url TEXT,
  fetched_at TEXT,
  confidence TEXT
);"#,
    )
    .await;
    exec_lenient(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS platform_behaviors (
  platform TEXT PRIMARY KEY,
  currency TEXT,
  price_display TEXT,
  source_url TEXT,
  fetched_at TEXT,
  confidence TEXT
);"#,
    )
    .await;
    exec_lenient(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS comparison_rules (
  id INTEGER PRIMARY KEY,
  rule TEXT,
  sort_order INTEGER,
  source_url TEXT,
  fetched_at TEXT,
  confidence TEXT
);"#,
    )
    .await;

    // Normalized destination reference tables.
    exec_lenient(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS destination_areas (
  slug TEXT,
  area_id TEXT,
  name TEXT,
  type TEXT,
  vibe TEXT,
  source_url TEXT,
  fetched_at TEXT,
  confidence TEXT,
  PRIMARY KEY (slug, area_id)
);"#,
    )
    .await;
    exec_lenient(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS destination_pois (
  slug TEXT,
  poi_id TEXT,
  title TEXT,
  area TEXT,
  nearest_station TEXT,
  duration_min INTEGER,
  booking_required INTEGER,
  booking_url TEXT,
  cost_estimate INTEGER,
  notes TEXT,
  hours TEXT,
  address TEXT,
  source_url TEXT,
  fetched_at TEXT,
  confidence TEXT,
  lat REAL,
  lon REAL,
  PRIMARY KEY (slug, poi_id)
);"#,
    )
    .await;
    exec_lenient(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS destination_clusters (
  slug TEXT,
  cluster_id TEXT,
  name TEXT,
  description TEXT,
  duration_min INTEGER,
  best_area TEXT,
  source_url TEXT,
  fetched_at TEXT,
  confidence TEXT,
  PRIMARY KEY (slug, cluster_id)
);"#,
    )
    .await;
    exec_lenient(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS destination_transit (
  slug TEXT,
  pair_key TEXT,
  kind TEXT,
  minutes INTEGER,
  line TEXT,
  station_from TEXT,
  station_to TEXT,
  source_url TEXT,
  fetched_at TEXT,
  confidence TEXT,
  PRIMARY KEY (slug, pair_key)
);"#,
    )
    .await;
    exec_lenient(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS destination_omiyage_items (
  slug TEXT,
  item_id TEXT,
  name TEXT,
  category TEXT,
  notes TEXT,
  source_url TEXT,
  fetched_at TEXT,
  confidence TEXT,
  PRIMARY KEY (slug, item_id)
);"#,
    )
    .await;
    exec_lenient(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS destination_omiyage_locations (
  slug TEXT,
  item_id TEXT,
  poi_id TEXT,
  purchase_note TEXT,
  source_url TEXT,
  fetched_at TEXT,
  confidence TEXT,
  PRIMARY KEY (slug, item_id, poi_id)
);"#,
    )
    .await;

    exec_lenient(
        &conn,
        "UPDATE destination_config SET ref_path = '' WHERE ref_path LIKE 'src/skills/%';",
    )
    .await;
    exec_lenient(&conn, "DROP TABLE IF EXISTS destination_references;").await;

    // Domestic accommodations (Phase A, Taiwan domestic stays — no JSON).
    exec_create(
        &conn,
        r#"CREATE TABLE IF NOT EXISTS domestic_accommodations (
  id TEXT PRIMARY KEY,
  destination TEXT NOT NULL,
  hotel_name TEXT NOT NULL,
  room_type TEXT NOT NULL,
  sea_view INTEGER NOT NULL CHECK(sea_view IN (0,1)),
  max_occupancy INTEGER,
  price_twd INTEGER NOT NULL,
  currency TEXT NOT NULL DEFAULT 'TWD',
  breakfast_included INTEGER NOT NULL CHECK(breakfast_included IN (0,1)),
  source TEXT,
  image_url TEXT,
  updated_at DATETIME NOT NULL DEFAULT (datetime('now'))
);"#,
    )
    .await;
    exec_lenient(
        &conn,
        "CREATE INDEX IF NOT EXISTS idx_domestic_accommodations_dest ON domestic_accommodations(destination);",
    )
    .await;
    // Back-compat: existing DBs created before image_url existed.
    add_column(
        &conn,
        "ALTER TABLE domestic_accommodations ADD COLUMN image_url TEXT;",
    )
    .await;
    // Manual seed for Jiufen Phase A examples (idempotent INSERT OR IGNORE).
    // image_url: local CSS placeholder for CHLIV/山城逸境 (no external via.placeholder.com — broken in Chrome 0x0); 海論 keeps its real URL.
    for (id, hotel, room, sea_view, price, breakfast, source, image_url) in [
        (
            "jiufen_hailun_seaview_5200",
            "海論",
            "海景雙人房",
            1,
            5200,
            1,
            "manual",
            "https://ocean-theory.h-and.world/assets/room-rouge-CBUWFFnr.webp",
        ),
        (
            "jiufen_chliv_seaview_7200",
            "CHLIV",
            "海景雙人房",
            1,
            7200,
            1,
            "manual",
            "",
        ),
        (
            "jiufen_shancheng_seaview_4200",
            "山城逸境",
            "海景雙人房",
            1,
            4200,
            1,
            "manual",
            "",
        ),
    ] {
        exec_lenient(
            &conn,
            &format!(
                "INSERT OR IGNORE INTO domestic_accommodations \
                 (id, destination, hotel_name, room_type, sea_view, price_twd, currency, breakfast_included, source, image_url, updated_at) \
                 VALUES ({}, {}, {}, {}, {sea_view}, {price}, 'TWD', {breakfast}, {}, {}, datetime('now'))",
                sq(id),
                sq("jiufen"),
                sq(hotel),
                sq(room),
                sq(source),
                sq(image_url),
            ),
        )
        .await;
        // Backfill image_url for pre-existing rows where it is still NULL/empty.
        exec_lenient(
            &conn,
            &format!(
                "UPDATE domestic_accommodations SET image_url = {} WHERE id = {} AND (image_url IS NULL OR image_url = '')",
                sq(image_url),
                sq(id),
            ),
        )
        .await;
        // Ensure destination_config contains jiufen so resolve_active_destination can pass for tests that use it.
        exec_lenient(
            &conn,
            &format!(
                "INSERT OR IGNORE INTO destination_config (slug, display_name, timezone, currency, language, origin) \
                 VALUES ({}, {}, 'Asia/Taipei', 'TWD', 'zh-TW', 'taiwan')",
                sq("jiufen"),
                sq("九份"),
            ),
        )
        .await;
    }
    // Cleanup legacy broken via.placeholder.com URLs (Chrome reports them as 0x0 broken images).
    // Existing DBs that already seeded the placeholder need this one-shot migration to local fallback.
    exec_lenient(
        &conn,
        "UPDATE domestic_accommodations SET image_url = '' WHERE image_url LIKE '%via.placeholder.com%'",
    )
    .await;

    // De-JSON batches (guarded — skip on already-migrated DB).
    de_json_reference_data(&conn).await;
    de_json_itinerary_sessions(&conn).await;
    de_json_plan_transport(&conn).await;
    de_json_cascade_schema(&conn).await;
    de_json_plan_events(&conn).await;
    de_json_shaping_knowledge(&conn).await;
    de_json_tour_group_raw_text(&conn).await;
    de_json_remaining_blobs(&conn).await;

    println!("Done.");
    Ok(())
}

// ---------------------------------------------------------------------------
// 15b. flights → flight_legs migration (no-op once `flights` is dropped).
// ---------------------------------------------------------------------------
async fn migrate_flights_to_legs(conn: &libsql::Connection) {
    if !table_exists(conn, "flights").await {
        return; // dead table already dropped (Batch F) — nothing to migrate.
    }
    // If flight_legs already has rows, skip (matches TS guard).
    if let Ok(mut rows) = conn.query("SELECT COUNT(*) AS cnt FROM flight_legs", ()).await {
        if let Ok(Some(row)) = rows.next().await {
            if let Ok(libsql::Value::Integer(n)) = row.get_value(0) {
                if n > 0 {
                    return;
                }
            }
        }
    }
    // The TS pass JSON-parses flights.outbound_json/return_json into flight_legs.
    // The live DB has no `flights` table (Batch F dropped it), so this branch is
    // unreachable in practice. Batch F's DROP TABLE IF EXISTS flights handles the
    // table removal; we do not attempt the JSON-parse port since there is no
    // surviving data shape to port against on the canonical DB.
}

// ---------------------------------------------------------------------------
// Seed backfills.
// ---------------------------------------------------------------------------

struct DestSeed {
    slug: &'static str,
    display_name: &'static str,
    ref_id: &'static str,
    timezone: &'static str,
    currency: &'static str,
    markets: &'static [&'static str],
    primary_airports: &'static [&'static str],
    language: &'static str,
    origin: &'static str,
    lat: f64,
    lon: f64,
}

const DESTINATIONS: &[DestSeed] = &[
    DestSeed { slug: "tokyo_2026", display_name: "Tokyo", ref_id: "tokyo", timezone: "Asia/Tokyo", currency: "JPY", markets: &["TW", "JP"], primary_airports: &["NRT", "HND"], language: "ja", origin: "taiwan", lat: 35.6762, lon: 139.6503 },
    DestSeed { slug: "nagoya_2026", display_name: "Nagoya", ref_id: "nagoya", timezone: "Asia/Tokyo", currency: "JPY", markets: &["TW", "JP"], primary_airports: &["NGO"], language: "ja", origin: "taiwan", lat: 35.1815, lon: 136.9066 },
    DestSeed { slug: "osaka_2026", display_name: "Osaka", ref_id: "osaka", timezone: "Asia/Tokyo", currency: "JPY", markets: &["TW", "JP"], primary_airports: &["KIX", "ITM"], language: "ja", origin: "taiwan", lat: 34.6937, lon: 135.5023 },
    DestSeed { slug: "osaka_kyoto_2026", display_name: "Osaka + Kyoto", ref_id: "osaka_kyoto", timezone: "Asia/Tokyo", currency: "JPY", markets: &["TW", "JP"], primary_airports: &["KIX", "ITM"], language: "ja", origin: "taiwan", lat: 34.6937, lon: 135.5023 },
    DestSeed { slug: "kyoto_2026", display_name: "Kyoto", ref_id: "kyoto", timezone: "Asia/Tokyo", currency: "JPY", markets: &["TW", "JP"], primary_airports: &["KIX"], language: "ja", origin: "taiwan", lat: 35.0116, lon: 135.7681 },
];

async fn backfill_destination_config(conn: &libsql::Connection) {
    for d in DESTINATIONS {
        let insert = format!(
            "INSERT OR IGNORE INTO destination_config (slug, display_name, ref_id, ref_path, timezone, currency, language, origin, lat, lon) VALUES ({}, {}, {}, '', {}, {}, {}, {}, {}, {})",
            sq(d.slug), sq(d.display_name), sq(d.ref_id), sq(d.timezone), sq(d.currency), sq(d.language), sq(d.origin), d.lat, d.lon,
        );
        if let Err(e) = exec(conn, &insert).await {
            eprintln!("⚠️  Could not backfill destination_config {}: {e}", d.slug);
            continue;
        }
        exec_lenient(conn, &format!("DELETE FROM destination_airports WHERE slug = {}", sq(d.slug))).await;
        for (i, a) in d.primary_airports.iter().enumerate() {
            exec_lenient(conn, &format!("INSERT INTO destination_airports (slug, airport, sort_order) VALUES ({}, {}, {})", sq(d.slug), sq(a), i)).await;
        }
        exec_lenient(conn, &format!("DELETE FROM destination_markets WHERE slug = {}", sq(d.slug))).await;
        for (i, m) in d.markets.iter().enumerate() {
            exec_lenient(conn, &format!("INSERT INTO destination_markets (slug, market, sort_order) VALUES ({}, {}, {})", sq(d.slug), sq(m), i)).await;
        }
    }
}

async fn backfill_origin_config(conn: &libsql::Connection) {
    exec_lenient(conn, "INSERT OR IGNORE INTO origin_config (slug, country_code, currency, timezone, holiday_calendar) VALUES ('taiwan', 'TW', 'TWD', 'Asia/Taipei', NULL)").await;
    exec_lenient(conn, "UPDATE origin_config SET holiday_calendar = NULL WHERE slug = 'taiwan' AND holiday_calendar LIKE 'data/holidays/%'").await;
    let taiwan_airports = ["TPE", "TSA", "RMQ", "KHH"];
    exec_lenient(conn, "DELETE FROM origin_airports WHERE slug = 'taiwan'").await;
    for (i, a) in taiwan_airports.iter().enumerate() {
        exec_lenient(conn, &format!("INSERT INTO origin_airports (slug, airport, sort_order) VALUES ('taiwan', '{a}', {i})")).await;
    }
}

async fn backfill_global_config(conn: &libsql::Connection) {
    for (k, v) in [("default_destination", "tokyo_2026"), ("default_origin", "taiwan")] {
        exec_lenient(conn, &format!("INSERT OR IGNORE INTO global_config (key, value) VALUES ('{k}', '{v}')")).await;
    }
}

struct OtaSeed {
    source_id: &'static str,
    name: &'static str,
    types: &'static [&'static str],
    status: &'static str,
    scraper_script: Option<&'static str>,
    regions: &'static [&'static str],
    url_template: Option<&'static str>,
    notes: Option<&'static str>,
}

const OTA_SOURCES: &[OtaSeed] = &[
    OtaSeed { source_id: "besttour", name: "喜鴻假期", types: &["package"], status: "active", scraper_script: None, regions: &["tokyo","kansai","hokkaido","kyushu","okinawa"], url_template: Some("https://www.besttour.com.tw/e_web/search?v=//////{region_id}///////"), notes: Some("PROVEN REAL (2026-06-26, gwebcdb agent-parse path). Group tours (跟團, NOT FIT). Real listing = /e_web/search?v=//////{region_id}/////// (東京=295, 關東=28, 北海道=26); region IDs from /e_web/country?v=12. Each product = fixed departure date in its 產品編號 code (price shows as '元起', avail '可售：N'). Scroll to lazy-load all products. NOT /e_web/DOM/ (404).") },
    OtaSeed { source_id: "liontravel", name: "雄獅旅遊", types: &["package","flight","hotel"], status: "active", scraper_script: Some("scripts/scrape_liontravel_dated.py"), regions: &["tokyo","kansai","hokkaido","okinawa"], url_template: Some("https://vacation.liontravel.com/search?Destination={dest_code}&FromDate={from_yyyymmdd}&ToDate={to_yyyymmdd}&Days={days}&roomlist={adults}-0-0"), notes: Some("FIT search via vacation.liontravel.com, group tour URL unknown") },
    OtaSeed { source_id: "tigerair", name: "台灣虎航", types: &["flight"], status: "active", scraper_script: Some("scripts/scrape_tigerair.py"), regions: &[], url_template: Some("https://www.tigerairtw.com"), notes: Some("DEFERRED (2026-06-27): redundant — google_flights already carries 台灣虎航 fares (TWD ~9,614+) among 15 airlines. No deep-link; booking.tigerairtw.com is an opaque Quasar SPA form (GUID field ids, no placeholders) → high-friction to drive for single-carrier data already covered. Revisit only if tigerair-direct LCC pricing is specifically wanted.") },
    OtaSeed { source_id: "lifetour", name: "五福旅遊", types: &["package","flight","hotel"], status: "active", scraper_script: Some("scripts/scrape_package.py"), regions: &["tokyo","kansai","hokkaido","kyushu","okinawa"], url_template: Some("https://tour.lifetour.com.tw/searchlist/{departure}/{region_code}"), notes: Some("Group tour listing at tour.lifetour.com.tw/searchlist/{departure}/{region}. Some packages include 伴自由 (semi-FIT) days.") },
    OtaSeed { source_id: "settour", name: "東南旅遊", types: &["package","flight","hotel"], status: "active", scraper_script: None, regions: &["tokyo","kansai","hokkaido","kyushu","okinawa"], url_template: Some("https://tour.settour.com.tw/search?destinationCode={dest_code}"), notes: Some("PROVEN REAL (2026-06-26, gwebcdb agent-parse path). FIT 機+酒 at fit.settour.com.tw → form-drive (機+酒 tab via #mobileSearch, arr autocomplete coax, set flight AND hotel dates) → /product/v2 returns the single lowest-price combo. NOTE the custom parse_settour mis-reads v2: it divides 機加酒未稅總價 by pax (got 13391) instead of the per-person 含稅 figure (15711), and grabs UI chrome as hotel — agent-parse the per-person-with-tax price + real hotel name.") },
    OtaSeed { source_id: "travel4u", name: "山富旅遊", types: &["package"], status: "active", scraper_script: None, regions: &["tokyo","kansai","hokkaido","kyushu","okinawa","nagoya"], url_template: Some("https://www.travel4u.com.tw/group/area/{area_code}/japan/"), notes: Some("PROVEN REAL (2026-06-26, gwebcdb agent-parse path). Group tours (跟團). Numeric area_code (NOT 'JP', which 404s): 41=東京｜東北, 40=大阪｜四國, 39=北海道, 42=九州, 43=沖繩, 63=北陸｜名古屋, 178=mixed; codes from the homepage's group/area japan links. Each product lists MULTIPLE departure dates + price '$N 起'; mostly 高雄出發.") },
    OtaSeed { source_id: "eztravel", name: "易遊網", types: &["package","flight","hotel"], status: "active", scraper_script: None, regions: &["tokyo","kansai","nagoya","okinawa","sapporo","fukuoka"], url_template: Some("https://packages.eztravel.com.tw/roundtrip-TPE-{dest_code}?checkin={depart_date}&checkout={return_date}&adult={pax}&child=0"), notes: Some("PROVEN REAL (2026-06-26, gwebcdb agent-parse path). Package (機加酒): the GET deep-link bounces to the search form — drive it (React-autocomplete coax + dpicker dates + 搜尋). Hotels may be in a different city than dest; LCC baggage typically NOT included.") },
    OtaSeed { source_id: "jalan", name: "じゃらん", types: &["hotel"], status: "inactive", scraper_script: None, regions: &[], url_template: Some("https://www.jalan.net"), notes: Some("Japan domestic OTA - for local hotel bookings") },
    OtaSeed { source_id: "rakuten_travel", name: "楽天トラベル", types: &["hotel","package"], status: "inactive", scraper_script: None, regions: &[], url_template: Some("https://travel.rakuten.co.jp"), notes: Some("Japan domestic OTA") },
    OtaSeed { source_id: "trip", name: "Trip.com", types: &["flight"], status: "active", scraper_script: Some("scripts/scrape_package.py"), regions: &[], url_template: Some("https://www.trip.com/flights/{origin}-to-{dest}/tickets-{origin_code}-{dest_code}?dcity={origin_code}&acity={dest_code}&ddate={depart_date}&flighttype=ow&class=y&quantity={pax}"), notes: Some("DEFERRED (2026-06-27): redundant flight coverage (google_flights proves the flight TYPE across 15 airlines) + login-wall risk on trip.com flight results (human sign-in via login_assist, session persists in profile). Roundtrip shows outbound only — scrape return as a separate one-way; prices USD→TWD (~32). Revisit only if trip.com-specific fares are needed.") },
    OtaSeed { source_id: "booking", name: "Booking.com", types: &["hotel"], status: "inactive", scraper_script: Some("scripts/scrape_package.py"), regions: &[], url_template: Some("https://www.booking.com/searchresults.{lang}.html?dest_id={dest_id}&dest_type=city&checkin={checkin}&checkout={checkout}&group_adults={adults}&no_rooms={rooms}&selected_currency={currency}"), notes: Some("Cloudflare bot protection. Use lang=zh-tw for Traditional Chinese. Initial search may not load results - retry or use direct hotel URL. Prices shown per night.") },
    OtaSeed { source_id: "agoda", name: "Agoda", types: &["hotel"], status: "active", scraper_script: None, regions: &[], url_template: Some("https://www.agoda.com/{hotel_slug}/hotel/{city_slug}-{country}.html?checkIn={checkin}&los={nights}&adults={adults}&rooms={rooms}&currency={currency}"), notes: Some("PROVEN REAL (2026-06-26, gwebcdb agent-parse path). Direct hotel page URLs work reliably — full pricing/reviews/amenities; SEARCH pages time out. Use the hotel_page URL template.") },
    OtaSeed { source_id: "skyscanner", name: "Skyscanner", types: &["flight"], status: "inactive", scraper_script: None, regions: &[], url_template: Some("https://www.skyscanner.com.tw"), notes: Some("Strong bot detection - returns captcha page. Use Google Flights instead for multi-airline price comparison.") },
    OtaSeed { source_id: "google_flights", name: "Google Flights", types: &["flight"], status: "active", scraper_script: None, regions: &[], url_template: Some("https://www.google.com/travel/flights?q=Flights+to+{dest}+from+{origin}+on+{depart_date}+through+{return_date}&curr={currency}&hl=zh-TW"), notes: Some("PROVEN REAL (2026-06-26, gwebcdb agent-parse path). Natural-language query URL; returns all-inclusive TWD prices + airline/times/duration/nonstop/CO2. No form interaction needed.") },
];

/// Cold-start bootstrap of the canonical catalog enums (product_types,
/// coverage_block_reasons) from the checked-in seed SQL. The SQL is embedded via
/// `include_str!` so it ships with the binary (no CWD-dependent runtime read), yet the
/// facts live in a version-controlled `.sql` file editable WITHOUT touching Rust — data,
/// not a `const` array of structs. All statements are `INSERT OR IGNORE`, so this never
/// overwrites a live row; it only fills an empty catalog.
async fn seed_ota_catalog(conn: &libsql::Connection) {
    const CATALOG_SEED: &str = include_str!("../../../../scripts/seed/ota_catalog.seed.sql");
    run_seed_file_stmts(conn, CATALOG_SEED).await;
}

/// Cold-start coverage matrix + provider region codes (DB-centric provider architecture,
/// spec 2026-06-29). Replaces the manual `set-ota-coverage` / `set-ota-region` CLI runs that
/// originally populated these tables but were NOT reproducible from the tree — so a fresh /
/// re-seeded DB would have rendered an empty `ota-status`. All statements are `INSERT OR IGNORE`:
/// fills an empty catalog, never clobbers a live edit. (Review finding F1, 2026-06-29.)
async fn seed_ota_coverage(conn: &libsql::Connection) {
    const COVERAGE_SEED: &str = include_str!("../../../../scripts/seed/ota_coverage.seed.sql");
    run_seed_file_stmts(conn, COVERAGE_SEED).await;
}

async fn seed_ota_workflow(conn: &libsql::Connection) {
    const WORKFLOW_SEED: &str = include_str!("../../../../scripts/seed/ota_source_workflow.seed.sql");
    run_seed_file_stmts(conn, WORKFLOW_SEED).await;
}

async fn seed_ota_url_param(conn: &libsql::Connection) {
    const URL_PARAM_SEED: &str =
        include_str!("../../../../scripts/seed/ota_source_url_param.seed.sql");
    run_seed_file_stmts(conn, URL_PARAM_SEED).await;
}

async fn seed_product_type_inputs(conn: &libsql::Connection) {
    const INPUTS_SEED: &str =
        include_str!("../../../../scripts/seed/product_type_inputs.seed.sql");
    run_seed_file_stmts(conn, INPUTS_SEED).await;
}

/// Run a checked-in seed SQL file statement-by-statement, insert-if-absent. Strips line comments
/// WITHIN each `;`-split chunk (a leading `-- ...` block must not cause the whole INSERT that
/// follows it to be skipped — the split-then-skip-if-starts-with-`--` bug).
async fn run_seed_file_stmts(conn: &libsql::Connection, seed_sql: &str) {
    for stmt in seed_sql.split(';') {
        let sql: String = stmt
            .lines()
            .filter(|l| !l.trim_start().starts_with("--"))
            .collect::<Vec<_>>()
            .join("\n");
        let s = sql.trim();
        if s.is_empty() {
            continue;
        }
        exec_lenient(conn, s).await;
    }
}

/// Phase-A of the two-phase `notes` migration (spec 2026-06-29 "Migration / data preservation"):
/// record each `ota_sources.notes` string + a sha256 checksum + disposition in
/// `ota_notes_migration_audit`, so Phase B (DROP `notes`, a later release) can confirm every note
/// was accounted for. The note text's single source of truth is the `OTA_SOURCES` array (read here),
/// not a duplicated copy in a SQL file. Every note is `normalized` — its facts already landed in the
/// typed coverage/region rows seeded above. `INSERT OR IGNORE` keeps any live (Codex-written) audit
/// row authoritative; a fresh DB gets a self-consistent row whose checksum matches its own raw_note.
/// (Review finding F1, 2026-06-29.)
async fn backfill_ota_notes_audit(conn: &libsql::Connection) {
    use sha2::{Digest, Sha256};
    // The audit table predates a uniqueness constraint, so de-dup any rows a prior backfill ran
    // before this guard existed (keep the lowest rowid per source_id). Idempotent.
    exec_lenient(
        conn,
        "DELETE FROM ota_notes_migration_audit \
         WHERE rowid NOT IN (SELECT MIN(rowid) FROM ota_notes_migration_audit GROUP BY source_id)",
    )
    .await;
    for s in OTA_SOURCES {
        let Some(note) = s.notes else { continue };
        let checksum = format!("{:x}", Sha256::digest(note.as_bytes()));
        // Insert-if-absent guarded by NOT EXISTS (the table has no PK to make INSERT OR IGNORE
        // dedupe), so re-running migrate never appends a duplicate audit row. (Finding F1.)
        let sql = format!(
            "INSERT INTO ota_notes_migration_audit \
             (source_id, raw_note, checksum, normalized_at, disposition) \
             SELECT {sid}, {note}, {chk}, CURRENT_TIMESTAMP, 'normalized' \
             WHERE NOT EXISTS (SELECT 1 FROM ota_notes_migration_audit WHERE source_id = {sid})",
            sid = sq(s.source_id),
            note = sq(note),
            chk = sq(&checksum),
        );
        exec_lenient(conn, &sql).await;
    }
}

/// Assign `dedup_key` (content fingerprint) to legacy `offers` rows. Oldest-first so the
/// FIRST-SEEN row is canonical; a row is stamped only when no other row already carries the
/// key, so pre-existing content duplicates keep `dedup_key IS NULL` — visible debt for an
/// explicit P2 collapse command (migrate NEVER deletes). Idempotent: re-runs find zero
/// stampable rows. (plan docs/plans/2026-08-29-offers-dedup-a-prime.md)
async fn backfill_offers_dedup_keys(conn: &libsql::Connection) {
    let sql = "SELECT id, scraped_at, source_id, type, name, price_per_person, currency, \
         destination, departure_date, return_date, nights, availability, hotel_name, airline, \
         flight_outbound, flight_return, offer_key \
         FROM offers WHERE dedup_key IS NULL ORDER BY scraped_at ASC, id ASC";
    let mut rows = match conn.query(sql, ()).await {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("⚠️  Could not read offers for dedup backfill: {e}");
            return;
        }
    };
    let mut stamped: u64 = 0;
    let mut skipped: u64 = 0;
    loop {
        let Some(row) = (
            match rows.next().await {
                Ok(row) => row,
                Err(e) => {
                    eprintln!("⚠️  Could not read dedup-backfill row: {e}");
                    break;
                }
            }
        ) else {
            break;
        };
        let text = |i: i32| -> Option<String> {
            match row.get_value(i) {
                Ok(libsql::Value::Text(s)) => Some(s),
                _ => None,
            }
        };
        let num = |i: i32| -> Option<i64> {
            match row.get_value(i) {
                Ok(libsql::Value::Integer(n)) => Some(n),
                _ => None,
            }
        };
        let (Some(id), Some(scraped_at)) = (text(0), text(1)) else {
            skipped += 1;
            continue;
        };
        let offer = travel_db::OfferRow {
            id: id.clone(),
            source_id: text(2).unwrap_or_default(),
            offer_type: text(3).unwrap_or_default(),
            name: text(4),
            price_per_person: num(5),
            currency: text(6),
            destination: text(7),
            departure_date: text(8),
            return_date: text(9),
            nights: num(10),
            availability: text(11),
            hotel_name: text(12),
            airline: text(13),
            flight_outbound: text(14),
            flight_return: text(15),
            offer_key: text(16),
            scraped_at: scraped_at.clone(),
            ..Default::default()
        };
        let key = travel_db::repo::offers::dedup_key(&offer);
        let upd = "UPDATE offers SET dedup_key = ?1, \
                   last_seen_at = COALESCE(last_seen_at, scraped_at) \
                   WHERE id = ?2 AND scraped_at = ?3 AND dedup_key IS NULL \
                   AND NOT EXISTS (SELECT 1 FROM offers o2 WHERE o2.dedup_key = ?1)";
        match conn
            .execute(upd, libsql::params![key.clone(), id.clone(), scraped_at])
            .await
        {
            Ok(1) => stamped += 1,
            Ok(_) => skipped += 1,
            Err(e) => eprintln!("⚠️  Could not stamp dedup_key on offer {id}: {e}"),
        }
    }
    println!("offers_dedup_backfill\tstamped={stamped} skipped_duplicate_content={skipped}");
}

async fn seed_ota_sources(conn: &libsql::Connection) {
    let opt = |v: Option<&str>| v.map(sq).unwrap_or_else(|| "NULL".to_string());
    for s in OTA_SOURCES {
        // Cold-start bootstrap only: once a source row exists, live DB edits made through
        // set-ota-source / set-ota-coverage are authoritative and migrate must not
        // reassert the historical Rust seed over them.
        let insert = format!(
            "INSERT INTO ota_sources (source_id, name, status, scraper_script, url_template, notes) \
             VALUES ({sid}, {name}, {status}, {script}, {url}, {notes}) \
             ON CONFLICT(source_id) DO NOTHING",
            sid = sq(s.source_id),
            name = sq(s.name),
            status = sq(s.status),
            script = opt(s.scraper_script),
            url = opt(s.url_template),
            notes = opt(s.notes),
        );
        exec_lenient(conn, &insert).await;
        for t in s.types {
            exec_lenient(conn, &format!("INSERT OR IGNORE INTO ota_source_types (source_id, type) VALUES ({}, {})", sq(s.source_id), sq(t))).await;
        }
        for r in s.regions {
            exec_lenient(conn, &format!("INSERT OR IGNORE INTO ota_source_regions (source_id, region) VALUES ({}, {})", sq(s.source_id), sq(r))).await;
        }
    }
}

// ---------------------------------------------------------------------------
// De-JSON batches. On the live (already-migrated) DB every legacy `*_json`
// column is gone, so the guard short-circuits and ONLY the idempotent CREATE
// TABLE IF NOT EXISTS / ADD COLUMN / DROP COLUMN statements run (all no-ops).
// The backfill SELECT/INSERT/UPDATE bodies operated purely on the legacy JSON
// columns; with those columns gone there is no data to port, matching the TS
// `if (!cols.includes('xxx_json')) skip` behavior. We replicate the schema
// (child tables, scalar columns) and drop steps faithfully.
// ---------------------------------------------------------------------------

// Batch A — reference-data child tables + legacy-json drops.
async fn de_json_reference_data(conn: &libsql::Connection) {
    for sql in [
        "CREATE TABLE IF NOT EXISTS destination_area_stations (slug TEXT, area_id TEXT, station TEXT, sort_order INTEGER, PRIMARY KEY (slug, area_id, sort_order));",
        "CREATE TABLE IF NOT EXISTS destination_area_best_for (slug TEXT, area_id TEXT, tag TEXT, sort_order INTEGER, PRIMARY KEY (slug, area_id, sort_order));",
        "CREATE TABLE IF NOT EXISTS destination_poi_tags (slug TEXT, poi_id TEXT, tag TEXT, sort_order INTEGER, PRIMARY KEY (slug, poi_id, sort_order));",
        "CREATE TABLE IF NOT EXISTS destination_cluster_pois (slug TEXT, cluster_id TEXT, poi_id TEXT, sort_order INTEGER, PRIMARY KEY (slug, cluster_id, sort_order));",
        "CREATE TABLE IF NOT EXISTS destination_tips (slug TEXT, tip TEXT, sort_order INTEGER, PRIMARY KEY (slug, sort_order));",
        "CREATE TABLE IF NOT EXISTS destination_airports (slug TEXT, airport TEXT, sort_order INTEGER, PRIMARY KEY (slug, sort_order));",
        "CREATE TABLE IF NOT EXISTS destination_markets (slug TEXT, market TEXT, sort_order INTEGER, PRIMARY KEY (slug, sort_order));",
        "CREATE TABLE IF NOT EXISTS origin_airports (slug TEXT, airport TEXT, sort_order INTEGER, PRIMARY KEY (slug, sort_order));",
    ] {
        exec_lenient(conn, sql).await;
    }
    // Backfill guard: TS backfills only when destination_areas.stations_json
    // still exists. On the canonical (de-JSON'd) DB it is gone → schema-only.
    if has_column(conn, "destination_areas", "stations_json").await {
        eprintln!("  ⚠️  legacy reference *_json columns still present; run the TS migrate for the data backfill.");
    }
    for (table, col) in [
        ("destination_areas", "stations_json"),
        ("destination_areas", "best_for_json"),
        ("destination_pois", "tags_json"),
        ("destination_clusters", "pois_json"),
        ("destination_config", "tips_json"),
        ("destination_config", "primary_airports_json"),
        ("destination_config", "markets_json"),
        ("origin_config", "primary_airports_json"),
        ("ota_sources", "type_json"),
        ("ota_sources", "regions_json"),
    ] {
        drop_column(conn, table, col).await;
    }
}

// Batch C — itinerary session/activity child tables + drops.
async fn de_json_itinerary_sessions(conn: &libsql::Connection) {
    exec_lenient(
        conn,
        "CREATE TABLE IF NOT EXISTS session_activities_zh (plan_id TEXT NOT NULL, destination TEXT NOT NULL, day_number INTEGER NOT NULL, session_type TEXT NOT NULL, sort_order INTEGER NOT NULL, activity TEXT NOT NULL, PRIMARY KEY (plan_id, destination, day_number, session_type, sort_order));",
    )
    .await;
    exec_lenient(
        conn,
        "CREATE TABLE IF NOT EXISTS activity_tags (activity_id TEXT NOT NULL, tag TEXT NOT NULL, updated_at DATETIME DEFAULT CURRENT_TIMESTAMP, PRIMARY KEY (activity_id, tag));",
    )
    .await;
    for (table, col) in [
        ("timesofday", "activities_zh_json"),
        ("timesofday", "meals_json"),
        ("timesofday", "meals_zh_json"),
        ("activities", "tags_json"),
    ] {
        drop_column(conn, table, col).await;
    }
}

// Batch D — plan offer/hotel/transfer/transport child tables + scalar cols + drops.
async fn de_json_plan_transport(conn: &libsql::Connection) {
    for sql in [
        "CREATE TABLE IF NOT EXISTS plan_offer_includes (plan_id TEXT NOT NULL, destination TEXT NOT NULL, offer_id TEXT NOT NULL, sort_order INTEGER NOT NULL, item TEXT NOT NULL, PRIMARY KEY (plan_id, destination, offer_id, sort_order));",
        "CREATE TABLE IF NOT EXISTS plan_offer_hotel_access (plan_id TEXT NOT NULL, destination TEXT NOT NULL, offer_id TEXT NOT NULL, sort_order INTEGER NOT NULL, line TEXT NOT NULL, PRIMARY KEY (plan_id, destination, offer_id, sort_order));",
        "CREATE TABLE IF NOT EXISTS hotel_access_lines (plan_id TEXT NOT NULL, destination TEXT NOT NULL, sort_order INTEGER NOT NULL, line TEXT NOT NULL, updated_at DATETIME DEFAULT CURRENT_TIMESTAMP, PRIMARY KEY (plan_id, destination, sort_order));",
        "CREATE TABLE IF NOT EXISTS airport_transfer_candidates (plan_id TEXT NOT NULL, destination TEXT NOT NULL, direction TEXT NOT NULL CHECK(direction IN ('arrival', 'departure')), candidate_id TEXT NOT NULL, title TEXT NOT NULL, route TEXT, duration_min INTEGER, price_yen INTEGER, schedule TEXT, booking_url TEXT, notes TEXT, sort_order INTEGER NOT NULL DEFAULT 0, updated_at DATETIME DEFAULT CURRENT_TIMESTAMP, PRIMARY KEY (plan_id, destination, direction, candidate_id));",
        "CREATE TABLE IF NOT EXISTS location_zone_candidates (plan_id TEXT NOT NULL, destination TEXT NOT NULL, sort_order INTEGER NOT NULL, slug TEXT NOT NULL, display_name TEXT, pros_text TEXT, cons_text TEXT, PRIMARY KEY (plan_id, destination, sort_order));",
        "CREATE TABLE IF NOT EXISTS transport_extra_candidates (plan_id TEXT NOT NULL, destination TEXT NOT NULL, direction TEXT NOT NULL, sort_order INTEGER NOT NULL, candidate_id TEXT, method TEXT, route TEXT, departure_time TEXT, arrival_time TEXT, duration_min INTEGER, cost_jpy INTEGER, transfers INTEGER, extra_text TEXT, PRIMARY KEY (plan_id, destination, direction, sort_order));",
    ] {
        exec_lenient(conn, sql).await;
    }
    for col in ["home_to_airport_status", "airport_to_hotel_status"] {
        add_column(conn, &format!("ALTER TABLE transportation_extras ADD COLUMN {col} TEXT;")).await;
    }
    for col in [
        "selected_id TEXT", "selected_title TEXT", "selected_route TEXT",
        "selected_duration_min INTEGER", "selected_price_yen INTEGER", "selected_schedule TEXT",
        "selected_booking_url TEXT", "selected_notes TEXT",
    ] {
        add_column(conn, &format!("ALTER TABLE airport_transfers ADD COLUMN {col};")).await;
    }
    for (table, col) in [
        ("plan_offers", "includes_json"),
        ("plan_offer_hotels", "access_json"),
        ("hotels", "access_json"),
        ("airport_transfers", "candidates_json"),
        ("airport_transfers", "selected_json"),
        ("accommodation_location_zone", "candidates_json"),
        ("transportation_extras", "home_to_airport_json"),
        ("transportation_extras", "airport_to_hotel_json"),
    ] {
        drop_column(conn, table, col).await;
    }
}

// Batch E pt1 — cascade/schema config child tables + scalar cols + drops.
async fn de_json_cascade_schema(conn: &libsql::Connection) {
    for sql in [
        "CREATE TABLE IF NOT EXISTS cascade_trigger_resets (trigger_id TEXT NOT NULL, sort_order INTEGER NOT NULL, target TEXT NOT NULL, PRIMARY KEY (trigger_id, sort_order));",
        "CREATE TABLE IF NOT EXISTS cascade_trigger_populate_map (trigger_id TEXT NOT NULL, source_path TEXT NOT NULL, target_path TEXT NOT NULL, PRIMARY KEY (trigger_id, source_path));",
        "CREATE TABLE IF NOT EXISTS plan_schema_contract_nodes (plan_id TEXT NOT NULL, sort_order INTEGER NOT NULL, node TEXT NOT NULL, PRIMARY KEY (plan_id, sort_order));",
        "CREATE TABLE IF NOT EXISTS plan_process_precedence_entries (plan_id TEXT NOT NULL, name TEXT NOT NULL, primary_process TEXT, mode TEXT, fallback_text TEXT, rules_text TEXT, PRIMARY KEY (plan_id, name));",
        "CREATE TABLE IF NOT EXISTS plan_date_anchor_flex_dates (plan_id TEXT NOT NULL, kind TEXT NOT NULL, sort_order INTEGER NOT NULL, date TEXT NOT NULL, PRIMARY KEY (plan_id, kind, sort_order));",
    ] {
        exec_lenient(conn, sql).await;
    }
    for col in ["condition_field TEXT", "condition_changed INTEGER"] {
        add_column(conn, &format!("ALTER TABLE cascade_triggers ADD COLUMN {col};")).await;
    }
    for col in ["flex_date_flexible INTEGER", "flex_reason TEXT"] {
        add_column(conn, &format!("ALTER TABLE plan_root_date_anchor ADD COLUMN {col};")).await;
    }
    for (table, col) in [
        ("cascade_triggers", "reset_json"),
        ("cascade_triggers", "condition_json"),
        ("cascade_triggers", "populate_map_json"),
        ("plan_schema_contract", "process_nodes_json"),
        ("plan_process_precedence", "precedence_json"),
        ("plan_root_date_anchor", "flexibility_json"),
    ] {
        drop_column(conn, table, col).await;
    }
}

// Batch E pt2 — unify event store into plan_events.
async fn de_json_plan_events(conn: &libsql::Connection) {
    exec_lenient(
        conn,
        "CREATE TABLE IF NOT EXISTS plan_events (plan_id TEXT NOT NULL, scope TEXT NOT NULL, destination TEXT NOT NULL DEFAULT '', process_id TEXT NOT NULL DEFAULT '', sort_order INTEGER NOT NULL, event TEXT, event_at TEXT, from_state TEXT, to_state TEXT, PRIMARY KEY (plan_id, scope, destination, process_id, sort_order));",
    )
    .await;
    exec_lenient(
        conn,
        "CREATE TABLE IF NOT EXISTS plan_event_data (plan_id TEXT NOT NULL, scope TEXT NOT NULL, destination TEXT NOT NULL DEFAULT '', process_id TEXT NOT NULL DEFAULT '', sort_order INTEGER NOT NULL, key TEXT NOT NULL, value TEXT, PRIMARY KEY (plan_id, scope, destination, process_id, sort_order, key));",
    )
    .await;
    exec_lenient(
        conn,
        "CREATE TABLE IF NOT EXISTS event_log_next_actions (plan_id TEXT NOT NULL, sort_order INTEGER NOT NULL, action TEXT NOT NULL, PRIMARY KEY (plan_id, sort_order));",
    )
    .await;
    for (table, col) in [
        ("event_log_state", "next_actions_json"),
        ("event_log_global_processes", "events_json"),
        ("event_log_dest_processes", "events_json"),
    ] {
        drop_column(conn, table, col).await;
    }
    exec_lenient(conn, "DROP TABLE IF EXISTS event_log_process_events;").await;
}

// Batch F — shaping/OTA-knowledge/bookings child tables + text cols + drops.
async fn de_json_shaping_knowledge(conn: &libsql::Connection) {
    for sql in [
        "CREATE TABLE IF NOT EXISTS booking_type_rules (booking_type TEXT NOT NULL, sort_order INTEGER NOT NULL, rule TEXT NOT NULL, PRIMARY KEY (booking_type, sort_order));",
        "CREATE TABLE IF NOT EXISTS hotel_area_keywords (region TEXT NOT NULL, area_type TEXT NOT NULL, sort_order INTEGER NOT NULL, keyword TEXT NOT NULL, PRIMARY KEY (region, area_type, sort_order));",
        "CREATE TABLE IF NOT EXISTS platform_behavior_quirks (platform TEXT NOT NULL, sort_order INTEGER NOT NULL, quirk TEXT NOT NULL, PRIMARY KEY (platform, sort_order));",
        "CREATE TABLE IF NOT EXISTS platform_behavior_baggage_labels (platform TEXT NOT NULL, label TEXT NOT NULL, description TEXT, PRIMARY KEY (platform, label));",
        "CREATE TABLE IF NOT EXISTS itinerary_transit_key_lines (plan_id TEXT NOT NULL, destination TEXT NOT NULL, lang TEXT NOT NULL, sort_order INTEGER NOT NULL, line TEXT NOT NULL, PRIMARY KEY (plan_id, destination, lang, sort_order));",
    ] {
        exec_lenient(conn, sql).await;
    }
    for (table, col) in [
        ("shaping_selected_offers", "raw_text"),
        ("shaping_selected_offers", "provenance_text"),
        ("shaping_tour_group_offers", "raw_text"),
        ("shaping_tour_group_offers", "parse_warnings_text"),
        ("shaping_research_artifacts", "payload_text"),
        ("bookings_current", "payload_text"),
        ("events", "data_text"),
        ("bookings_events", "event_data_text"),
        ("itinerary_metadata", "transit_hotel_station"),
        ("itinerary_metadata", "transit_hotel_station_zh"),
    ] {
        add_column(conn, &format!("ALTER TABLE {table} ADD COLUMN {col} TEXT;")).await;
    }
    for (table, col) in [
        ("booking_types", "rules_json"),
        ("hotel_areas", "keywords_json"),
        ("platform_behaviors", "quirks_json"),
        ("platform_behaviors", "baggage_labels_json"),
        ("shaping_selected_offers", "raw_json"),
        ("shaping_selected_offers", "provenance_json"),
        ("shaping_tour_group_offers", "raw_json"),
        ("shaping_tour_group_offers", "parse_warnings_json"),
        ("shaping_research_artifacts", "payload_json"),
        ("bookings_current", "payload_json"),
        ("events", "data"),
        ("bookings_events", "event_data"),
        ("itinerary_metadata", "transit_summary"),
        ("itinerary_metadata", "transit_summary_zh"),
    ] {
        drop_column(conn, table, col).await;
    }
    exec_lenient(conn, "DROP TABLE IF EXISTS flights;").await;
}

// Batch F.2 — shaping_tour_group_offers raw_text → typed cols + notes child.
async fn de_json_tour_group_raw_text(conn: &libsql::Connection) {
    for col in [
        "raw_confidence", "raw_note", "raw_flight", "raw_flight_outbound", "raw_flight_return",
    ] {
        add_column(conn, &format!("ALTER TABLE shaping_tour_group_offers ADD COLUMN {col} TEXT;")).await;
    }
    exec_lenient(
        conn,
        "CREATE TABLE IF NOT EXISTS shaping_tour_group_offer_notes (run_id TEXT NOT NULL, offer_id TEXT NOT NULL, sort_order INTEGER NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY (run_id, offer_id, sort_order));",
    )
    .await;
    drop_column(conn, "shaping_tour_group_offers", "raw_text").await;
}

// Batch F.3 — remaining open-blob *_text columns → notes child tables + drops.
async fn de_json_remaining_blobs(conn: &libsql::Connection) {
    for sql in [
        "CREATE TABLE IF NOT EXISTS shaping_selected_offer_notes (selection_id TEXT NOT NULL, source TEXT NOT NULL, sort_order INTEGER NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY (selection_id, source, sort_order));",
        "CREATE TABLE IF NOT EXISTS shaping_research_artifact_notes (artifact_id TEXT NOT NULL, sort_order INTEGER NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY (artifact_id, sort_order));",
        "CREATE TABLE IF NOT EXISTS bookings_current_payload (booking_key TEXT NOT NULL, sort_order INTEGER NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY (booking_key, sort_order));",
        "CREATE TABLE IF NOT EXISTS bookings_event_data (booking_key TEXT NOT NULL, event_at TEXT NOT NULL, sort_order INTEGER NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY (booking_key, event_at, sort_order));",
    ] {
        exec_lenient(conn, sql).await;
    }
    // events.data_text stays (text contract). Drop the now-normalized blob cols.
    for (table, col) in [
        ("shaping_selected_offers", "raw_text"),
        ("shaping_selected_offers", "provenance_text"),
        ("shaping_research_artifacts", "payload_text"),
        ("bookings_current", "payload_text"),
        ("bookings_events", "event_data_text"),
    ] {
        drop_column(conn, table, col).await;
    }
}

// ---------------------------------------------------------------------------
// Static DDL blocks.
// ---------------------------------------------------------------------------

const ITINERARY_TABLES: &[&str] = &[
    r#"CREATE TABLE IF NOT EXISTS days (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  day_number INTEGER NOT NULL,
  date TEXT NOT NULL,
  theme TEXT,
  day_type TEXT NOT NULL CHECK(day_type IN ('arrival', 'full', 'departure')),
  status TEXT NOT NULL DEFAULT 'draft' CHECK(status IN ('draft', 'planned', 'confirmed')),
  weather_label TEXT,
  temp_low_c REAL,
  temp_high_c REAL,
  precipitation_pct REAL,
  weather_code INTEGER,
  feels_like_low_c REAL,
  feels_like_high_c REAL,
  weather_source_id TEXT,
  weather_sourced_at TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, day_number)
);"#,
    r#"CREATE TABLE IF NOT EXISTS timesofday (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  day_number INTEGER NOT NULL,
  session_type TEXT NOT NULL CHECK(session_type IN ('morning', 'noon', 'afternoon', 'evening')),
  focus TEXT,
  transit_notes TEXT,
  booking_notes TEXT,
  time_range_start TEXT,
  time_range_end TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, day_number, session_type)
);"#,
    r#"CREATE TABLE IF NOT EXISTS activities (
  id TEXT PRIMARY KEY,
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  poi_id TEXT,
  day_number INTEGER NOT NULL,
  session_type TEXT NOT NULL CHECK(session_type IN ('morning', 'noon', 'afternoon', 'evening')),
  sort_order INTEGER NOT NULL DEFAULT 0,
  title TEXT NOT NULL,
  area TEXT,
  nearest_station TEXT,
  duration_min INTEGER,
  booking_required INTEGER NOT NULL DEFAULT 0,
  booking_url TEXT,
  booking_status TEXT CHECK(booking_status IN ('not_required', 'pending', 'booked', 'waitlist')),
  booking_ref TEXT,
  book_by TEXT,
  start_time TEXT,
  end_time TEXT,
  is_fixed_time INTEGER NOT NULL DEFAULT 0,
  cost_estimate INTEGER,
  notes TEXT,
  priority TEXT NOT NULL DEFAULT 'want' CHECK(priority IN ('must', 'want', 'optional')),
  source TEXT NOT NULL DEFAULT 'confirmed' CHECK(source IN ('confirmed', 'ai_recommended')),
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);"#,
    r#"CREATE TABLE IF NOT EXISTS plan_metadata (
  plan_id TEXT PRIMARY KEY,
  schema_version TEXT NOT NULL,
  active_destination TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);"#,
    r#"CREATE TABLE IF NOT EXISTS date_anchors (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  start_date TEXT NOT NULL,
  end_date TEXT NOT NULL,
  days INTEGER NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
);"#,
    r#"CREATE TABLE IF NOT EXISTS process_statuses (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  process_id TEXT NOT NULL,
  status TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, process_id)
);"#,
    r#"CREATE TABLE IF NOT EXISTS cascade_dirty_flags (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  process_id TEXT NOT NULL,
  dirty INTEGER NOT NULL DEFAULT 0,
  last_changed DATETIME,
  PRIMARY KEY (plan_id, destination, process_id)
);"#,
    r#"CREATE TABLE IF NOT EXISTS airport_transfers (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  direction TEXT NOT NULL CHECK(direction IN ('arrival', 'departure')),
  status TEXT NOT NULL DEFAULT 'planned' CHECK(status IN ('planned', 'booked')),
  selected_id TEXT, selected_title TEXT, selected_route TEXT,
  selected_duration_min INTEGER, selected_price_yen INTEGER, selected_schedule TEXT,
  selected_booking_url TEXT, selected_notes TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, direction)
);"#,
    r#"CREATE TABLE IF NOT EXISTS flights (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  populated_from TEXT,
  airline TEXT,
  airline_code TEXT,
  outbound_json TEXT,
  return_json TEXT,
  booked_date TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
);"#,
    r#"CREATE TABLE IF NOT EXISTS hotels (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  populated_from TEXT,
  name TEXT,
  check_in TEXT,
  notes TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
);"#,
];

const PHASE1_TABLES: &[&str] = &[
    r#"CREATE TABLE IF NOT EXISTS plan_destinations (
  plan_id TEXT NOT NULL, slug TEXT NOT NULL, display_name TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'draft',
  created_at DATETIME, updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, slug)
)"#,
    r#"CREATE TABLE IF NOT EXISTS destination_details (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  origin_city TEXT, region TEXT, primary_airport TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
)"#,
    r#"CREATE TABLE IF NOT EXISTS destination_cities (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL, city_slug TEXT NOT NULL,
  role TEXT NOT NULL, nights INTEGER,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, city_slug)
)"#,
    r#"CREATE TABLE IF NOT EXISTS plan_offers (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL, id TEXT NOT NULL,
  source_id TEXT NOT NULL, type TEXT NOT NULL, title TEXT,
  price_per_person INTEGER, currency TEXT DEFAULT 'TWD', availability TEXT,
  url TEXT, scraped_at TEXT, product_code TEXT, duration_days INTEGER,
  price_total INTEGER, seats_remaining INTEGER,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, id)
)"#,
    r#"CREATE TABLE IF NOT EXISTS plan_offer_flights (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL, offer_id TEXT NOT NULL,
  direction TEXT NOT NULL CHECK(direction IN ('outbound', 'return')),
  flight_number TEXT, airline TEXT, airline_code TEXT,
  departure_code TEXT, departure_time TEXT, arrival_code TEXT, arrival_time TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, offer_id, direction)
)"#,
    r#"CREATE TABLE IF NOT EXISTS plan_offer_hotels (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL, offer_id TEXT NOT NULL,
  name TEXT, slug TEXT, area TEXT, star_rating INTEGER,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, offer_id)
)"#,
    r#"CREATE TABLE IF NOT EXISTS plan_offer_date_pricing (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL, offer_id TEXT NOT NULL,
  date TEXT NOT NULL, price INTEGER NOT NULL, availability TEXT, seats_remaining INTEGER,
  currency TEXT DEFAULT 'TWD',
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, offer_id, date)
)"#,
    r#"CREATE TABLE IF NOT EXISTS plan_offer_best_value (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL, offer_id TEXT NOT NULL,
  best_date TEXT, best_price INTEGER, currency TEXT DEFAULT 'TWD',
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, offer_id)
)"#,
    r#"CREATE TABLE IF NOT EXISTS plan_offer_selection (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  selected_offer_id TEXT, selected_date TEXT, selected_at DATETIME,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
)"#,
    r#"CREATE TABLE IF NOT EXISTS plan_offer_provenance (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  source_id TEXT NOT NULL, scraped_at TEXT NOT NULL,
  file_path TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, source_id, scraped_at)
)"#,
    r#"CREATE TABLE IF NOT EXISTS plan_offer_warnings (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  plan_id TEXT NOT NULL, destination TEXT NOT NULL, offer_id TEXT,
  warning_type TEXT NOT NULL, message TEXT NOT NULL,
  created_at DATETIME DEFAULT CURRENT_TIMESTAMP
)"#,
    r#"CREATE TABLE IF NOT EXISTS plan_budget (
  plan_id TEXT PRIMARY KEY,
  total_cap INTEGER, flight_cap INTEGER, accommodation_cap INTEGER, daily_cap INTEGER,
  pax INTEGER NOT NULL DEFAULT 1, currency TEXT DEFAULT 'TWD',
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
)"#,
    r#"CREATE TABLE IF NOT EXISTS cascade_triggers (
  plan_id TEXT NOT NULL, trigger_id TEXT NOT NULL,
  event TEXT NOT NULL, scope TEXT NOT NULL,
  condition_field TEXT, condition_changed INTEGER, action TEXT, set_source TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, trigger_id)
)"#,
    r#"CREATE TABLE IF NOT EXISTS plan_schema_contract (
  plan_id TEXT PRIMARY KEY,
  id_convention TEXT NOT NULL, currency TEXT NOT NULL DEFAULT 'TWD',
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
)"#,
    r#"CREATE TABLE IF NOT EXISTS plan_process_precedence (
  plan_id TEXT PRIMARY KEY,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
)"#,
    r#"CREATE TABLE IF NOT EXISTS cascade_global_state (
  plan_id TEXT PRIMARY KEY,
  last_cascade_run DATETIME, active_dest_last TEXT, p1_dirty INTEGER NOT NULL DEFAULT 0,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
)"#,
    r#"CREATE TABLE IF NOT EXISTS plan_root_date_anchor (
  plan_id TEXT PRIMARY KEY,
  status TEXT NOT NULL, set_out_date TEXT, duration_days INTEGER, return_date TEXT,
  flex_date_flexible INTEGER, flex_reason TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
)"#,
    r#"CREATE TABLE IF NOT EXISTS itinerary_metadata (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  scaffolded_at DATETIME, populated_at DATETIME, transit_summary TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
)"#,
    r#"CREATE TABLE IF NOT EXISTS accommodation_location_zone (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  selected_area TEXT, source TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
)"#,
    r#"CREATE TABLE IF NOT EXISTS transportation_extras (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  source TEXT, populated_from TEXT, research_notes TEXT,
  home_to_airport_status TEXT, airport_to_hotel_status TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
)"#,
    r#"CREATE TABLE IF NOT EXISTS event_log_state (
  plan_id TEXT PRIMARY KEY,
  session TEXT NOT NULL, project TEXT NOT NULL, version TEXT NOT NULL,
  current_focus TEXT, active_destination TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
)"#,
    r#"CREATE TABLE IF NOT EXISTS event_log_global_processes (
  plan_id TEXT NOT NULL, process_id TEXT NOT NULL,
  status TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, process_id)
)"#,
    r#"CREATE TABLE IF NOT EXISTS event_log_destinations (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  status TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination)
)"#,
    r#"CREATE TABLE IF NOT EXISTS event_log_dest_processes (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL, process_id TEXT NOT NULL,
  status TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, process_id)
)"#,
    r#"CREATE TABLE IF NOT EXISTS airport_transfer_candidates (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  direction TEXT NOT NULL CHECK(direction IN ('arrival', 'departure')),
  candidate_id TEXT NOT NULL,
  title TEXT NOT NULL, route TEXT, duration_min INTEGER, price_yen INTEGER,
  schedule TEXT, booking_url TEXT, notes TEXT, sort_order INTEGER NOT NULL DEFAULT 0,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, direction, candidate_id)
)"#,
    r#"CREATE TABLE IF NOT EXISTS hotel_access_lines (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  sort_order INTEGER NOT NULL, line TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, sort_order)
)"#,
    r#"CREATE TABLE IF NOT EXISTS session_meals (
  plan_id TEXT NOT NULL, destination TEXT NOT NULL,
  day_number INTEGER NOT NULL, session_type TEXT NOT NULL,
  sort_order INTEGER NOT NULL, meal TEXT NOT NULL,
  source TEXT NOT NULL DEFAULT 'confirmed' CHECK(source IN ('confirmed', 'ai_recommended')),
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (plan_id, destination, day_number, session_type, sort_order)
)"#,
    r#"CREATE TABLE IF NOT EXISTS activity_tags (
  activity_id TEXT NOT NULL, tag TEXT NOT NULL,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (activity_id, tag)
)"#,
    // Per-plan dashboard share tokens (view scope). Worker reads this; the CLI
    // `share-token` command is the sole write path. Opaque token is the PK.
    r#"CREATE TABLE IF NOT EXISTS plan_share_tokens (
  plan_id TEXT NOT NULL,
  token TEXT NOT NULL PRIMARY KEY,
  created_at TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'active',
  created_by TEXT,
  deactivated_at TEXT,
  deactivated_by TEXT
)"#,
];

const SHAPING_RESEARCH_TABLES: &[&str] = &[
    r#"CREATE TABLE IF NOT EXISTS shaping_research_runs (
  run_id TEXT PRIMARY KEY,
  origin_code TEXT NOT NULL,
  pax INTEGER NOT NULL,
  window_start TEXT NOT NULL,
  window_end TEXT NOT NULL,
  currency TEXT NOT NULL,
  exchange_rate_usd_twd REAL NOT NULL,
  status TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);"#,
    r#"CREATE TABLE IF NOT EXISTS shaping_research_destinations (
  run_id TEXT NOT NULL,
  dest_code TEXT NOT NULL,
  dest_label TEXT NOT NULL,
  sort_order INTEGER NOT NULL,
  PRIMARY KEY (run_id, dest_code)
);"#,
    r#"CREATE TABLE IF NOT EXISTS shaping_research_durations (
  run_id TEXT NOT NULL,
  nights INTEGER NOT NULL,
  duration_days INTEGER NOT NULL,
  PRIMARY KEY (run_id, nights)
);"#,
    r#"CREATE TABLE IF NOT EXISTS shaping_candidates (
  candidate_id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL,
  dest_code TEXT NOT NULL,
  depart_date TEXT NOT NULL,
  return_date TEXT NOT NULL,
  nights INTEGER NOT NULL,
  flight_total_twd INTEGER,
  leave_days INTEGER,
  rank INTEGER,
  verdict TEXT,
  adopted_plan_id TEXT
);"#,
    r#"CREATE TABLE IF NOT EXISTS shaping_candidate_flights (
  candidate_id TEXT NOT NULL,
  direction TEXT NOT NULL,
  airline TEXT,
  depart_time TEXT,
  arrive_time TEXT,
  duration TEXT,
  nonstop INTEGER,
  price_total_twd INTEGER,
  PRIMARY KEY (candidate_id, direction)
);"#,
    r#"CREATE TABLE IF NOT EXISTS shaping_scrape_attempts (
  run_id TEXT NOT NULL,
  dest_code TEXT NOT NULL,
  nights INTEGER NOT NULL,
  status TEXT NOT NULL,
  candidate_count INTEGER,
  error TEXT,
  attempted_at TEXT,
  PRIMARY KEY (run_id, dest_code, nights)
);"#,
];

#[cfg(test)]
mod ota_coverage_backfill_tests {
    //! Unit tests for review finding F1: the OTA coverage matrix + notes-audit must be reproducible
    //! FROM CODE (the checked-in seed file + `backfill_ota_notes_audit`), and the backfill must be
    //! idempotent (the shipped Task-7 audit table has no PK, so a naive INSERT duplicated rows on
    //! every migrate). In-memory libsql — never touches the shared production Turso DB.

    use super::*;

    async fn mem_conn() -> libsql::Connection {
        libsql::Builder::new_local(":memory:")
            .build()
            .await
            .expect("in-memory db")
            .connect()
            .expect("connect")
    }

    async fn create_catalog_tables(conn: &libsql::Connection) {
        for ddl in [
            "CREATE TABLE ota_source_coverage (source_id TEXT NOT NULL, product_type TEXT NOT NULL, \
             proven INTEGER NOT NULL DEFAULT 0, proven_at TEXT, method TEXT, search_url TEXT, \
             blocked_reason_code TEXT, updated_at DATETIME DEFAULT CURRENT_TIMESTAMP, \
             PRIMARY KEY (source_id, product_type))",
            "CREATE TABLE ota_source_region_codes (source_id TEXT NOT NULL, product_type TEXT NOT NULL, \
             region_label TEXT NOT NULL, region_code TEXT NOT NULL, \
             PRIMARY KEY (source_id, product_type, region_label))",
            "CREATE TABLE ota_notes_migration_audit (source_id TEXT, raw_note TEXT, checksum TEXT, \
             normalized_at TEXT, disposition TEXT)",
        ] {
            conn.execute(ddl, ()).await.unwrap();
        }
    }

    async fn scalar_i64(conn: &libsql::Connection, sql: &str) -> i64 {
        let mut rows = conn.query(sql, ()).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        match row.get_value(0).unwrap() {
            libsql::Value::Integer(n) => n,
            _ => panic!("not an integer"),
        }
    }

    #[tokio::test]
    async fn seed_and_backfill_populate_from_code() {
        let conn = mem_conn().await;
        create_catalog_tables(&conn).await;

        seed_ota_coverage(&conn).await;
        backfill_ota_notes_audit(&conn).await;

        // Coverage matrix came from the checked-in seed file (all 14 sources, the 6 proven ones).
        assert_eq!(
            scalar_i64(&conn, "SELECT count(*) FROM ota_source_coverage").await,
            14,
            "seed file must populate the full coverage matrix"
        );
        assert_eq!(
            scalar_i64(&conn, "SELECT count(*) FROM ota_source_coverage WHERE proven=1").await,
            6,
            "the 6 proven sources must be seeded proven=1"
        );
        assert_eq!(
            scalar_i64(
                &conn,
                "SELECT proven FROM ota_source_coverage WHERE source_id='settour' AND product_type='fit'"
            )
            .await,
            1,
        );
        // Region codes seeded (besttour 3 + travel4u 7).
        assert_eq!(
            scalar_i64(&conn, "SELECT count(*) FROM ota_source_region_codes").await,
            10,
        );
        // Notes audit backfilled (one per source with a note; all 14 OtaSeed entries have notes).
        assert_eq!(
            scalar_i64(&conn, "SELECT count(*) FROM ota_notes_migration_audit").await,
            14,
        );
        assert_eq!(
            scalar_i64(
                &conn,
                "SELECT count(*) FROM ota_notes_migration_audit WHERE disposition='normalized'"
            )
            .await,
            14,
        );
    }

    #[tokio::test]
    async fn backfill_is_idempotent_no_duplicate_audit_rows() {
        let conn = mem_conn().await;
        create_catalog_tables(&conn).await;

        // The F1 bug: re-running migrate appended duplicate audit rows (table has no PK).
        backfill_ota_notes_audit(&conn).await;
        backfill_ota_notes_audit(&conn).await;
        backfill_ota_notes_audit(&conn).await;

        assert_eq!(
            scalar_i64(&conn, "SELECT count(*) FROM ota_notes_migration_audit").await,
            14,
            "backfill must be idempotent — exactly one audit row per source after repeated runs"
        );
        assert_eq!(
            scalar_i64(
                &conn,
                "SELECT count(DISTINCT source_id) FROM ota_notes_migration_audit"
            )
            .await,
            14,
        );
    }

    #[tokio::test]
    async fn backfill_dedups_preexisting_duplicate_rows() {
        let conn = mem_conn().await;
        create_catalog_tables(&conn).await;
        // Simulate the corrupted live state: many duplicate rows for one source.
        for _ in 0..5 {
            conn.execute(
                "INSERT INTO ota_notes_migration_audit (source_id, disposition) VALUES ('settour','normalized')",
                (),
            )
            .await
            .unwrap();
        }
        assert_eq!(
            scalar_i64(&conn, "SELECT count(*) FROM ota_notes_migration_audit WHERE source_id='settour'").await,
            5,
        );

        backfill_ota_notes_audit(&conn).await;

        assert_eq!(
            scalar_i64(&conn, "SELECT count(*) FROM ota_notes_migration_audit WHERE source_id='settour'").await,
            1,
            "backfill must collapse pre-existing duplicate audit rows to one per source"
        );
    }
}

#[cfg(test)]
mod offers_dedup_backfill_tests {
    //! In-memory libsql tests for `backfill_offers_dedup_keys`: stamps oldest-first, keeps the
    //! first-seen row canonical when pre-existing content duplicates exist (newer duplicate
    //! stays NULL for the explicit P2 collapse), never deletes, and is idempotent on re-run.

    use super::*;

    async fn mem_conn() -> libsql::Connection {
        libsql::Builder::new_local(":memory:")
            .build()
            .await
            .expect("in-memory db")
            .connect()
            .expect("connect")
    }

    async fn create_offers(conn: &libsql::Connection) {
        conn.execute(
            "CREATE TABLE offers (id TEXT, source_id TEXT, type TEXT, name TEXT, \
             price_per_person INTEGER, currency TEXT, region TEXT, destination TEXT, \
             departure_date TEXT, return_date TEXT, nights INTEGER, availability TEXT, \
             hotel_name TEXT, airline TEXT, flight_outbound TEXT, flight_return TEXT, \
             scraped_at TEXT, offer_key TEXT, dedup_key TEXT, last_seen_at TEXT, \
             PRIMARY KEY (id, scraped_at))",
            (),
        )
        .await
        .expect("create offers");
    }

    async fn ins(conn: &libsql::Connection, id: &str, scraped_at: &str, hotel: &str, price: i64) {
        conn.execute(
            "INSERT INTO offers (id, source_id, type, destination, departure_date, nights, \
             hotel_name, price_per_person, currency, scraped_at) \
             VALUES (?1,'zztest','package','tokyo_2026','2026-09-01',3,?2,?3,'TWD',?4)",
            libsql::params![id, hotel, price, scraped_at],
        )
        .await
        .expect("insert offer");
    }

    async fn count(conn: &libsql::Connection, sql: &str) -> i64 {
        let mut rows = conn.query(sql, ()).await.expect("count query");
        let row = rows.next().await.expect("a row").expect("a row");
        match row.get_value(0) {
            Ok(libsql::Value::Integer(n)) => n,
            other => panic!("expected integer count, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn stamps_oldest_first_and_skips_preexisting_duplicates() {
        let conn = mem_conn().await;
        create_offers(&conn).await;
        // Two identical-content rows (the legacy duplicate shape) + one price change.
        ins(&conn, "zzdup", "2026-09-01T08:00:00Z", "Hotel Alpha", 25500).await;
        ins(&conn, "zzdup2", "2026-09-05T08:00:00Z", "Hotel Alpha", 25500).await;
        ins(&conn, "zzchg", "2026-09-06T08:00:00Z", "Hotel Alpha", 26900).await;
        let before = count(&conn, "SELECT COUNT(*) FROM offers").await;
        backfill_offers_dedup_keys(&conn).await;
        // Oldest duplicate canonical, newer duplicate left NULL, price change gets its own key.
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM offers WHERE dedup_key IS NOT NULL").await,
            2
        );
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM offers WHERE dedup_key IS NULL").await,
            1
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM offers WHERE dedup_key IS NOT NULL \
                 AND id = 'zzdup' AND last_seen_at = '2026-09-01T08:00:00Z'"
            )
            .await,
            1,
            "first-seen row canonical, last_seen_at = scraped_at"
        );
        // Never deletes.
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM offers").await, before);
    }

    #[tokio::test]
    async fn is_idempotent_on_rerun() {
        let conn = mem_conn().await;
        create_offers(&conn).await;
        ins(&conn, "zzone", "2026-09-01T08:00:00Z", "Hotel Beta", 10000).await;
        backfill_offers_dedup_keys(&conn).await;
        let stamped =
            count(&conn, "SELECT COUNT(*) FROM offers WHERE dedup_key IS NOT NULL").await;
        assert_eq!(stamped, 1);
        backfill_offers_dedup_keys(&conn).await;
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM offers WHERE dedup_key IS NOT NULL").await,
            stamped
        );
    }
}
