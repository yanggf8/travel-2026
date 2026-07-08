# Map-Coverage Guardrail + `set-activity-poi --auto` Implementation Plan

## Goal

Two complementary CLI changes so the flow detects the empty-map-day gap automatically and lets the
agent fix it in one command instead of N manual links:

- **A (detect):** a per-day map-coverage **WARN** in `validate publish`.
- **B (fix):** a `set-activity-poi --auto [--dest]` batch mode that links `poi_id=NULL` activities to
  their `destination_pois` match, deterministically, never guessing.

## Architecture

Rust single-binary CLI:

```text
./bin/travel <cmd> [args]
        -> rust/crates/travel-cli/src/main.rs
        -> command module run(args, plan_id)
        -> crate::db::connect_read|write()
        -> Turso normalized tables
```

- `validate publish` is read-only and lives in `rust/crates/travel-cli/src/validate.rs`.
- `set-activity-poi` is a mutation command in `rust/crates/travel-cli/src/set_activity_poi.rs`.
- Domain activity POI writes go through `travel_db::repo::itinerary::set_activity_poi`.
- Mutation audit back-half goes through `cascade::common::record_operation`.
- Integration tests live in `rust/crates/travel-cli/tests/*.rs` and use
  `rust/crates/travel-cli/tests/common/mod.rs`.

## Tech Stack

- Rust workspace under `rust/`.
- `travel-cli` integration tests using the shared `common::` harness.
- Turso/libSQL as the source of truth.
- Plain-text CLI output only. No JSON output, no JSON fixtures, and no JSON pipeline boundary.

## Global Constraints

- **Agent-first plain text only**: CLI stdout is plain text / table lines, never JSON.
- **Fail loud, no local fallback**: read source of truth from Turso; a missing row throws.
- **No fabricated coords**: never invent lat/lon; only link POIs that are already geocoded.
- **Audit triad on every mutation**: `plan_events` + `plan_event_data` + `operation_runs` +
  `plans.version`, with `record_operation` called once per logical mutation.
- **Reuse existing SQL/helpers**: Part A reuses the `has_map_path` POI-join predicate; Part B reuses
  `set_activity_poi`'s existing resolve/assert/audit machinery and `itinerary::set_activity_poi`.
- **Tests**: real-Turso behavior-lock integration tests on `common::bin`, `common::db_exec -> Option<Rows>`,
  `common::seed_plan(plan, dest, version)`, `common::teardown_plan`, and RAII `Guard`.
- **Serialized background test runs**: use
  `cargo test -p travel-cli --test <name> -- --test-threads=1` in the background. A foreground timeout can
  SIGTERM the test before `Guard::drop` runs.
- **Non-plan-keyed teardown**: `destination_pois` is slug-keyed, not plan-keyed. Tests that seed it must
  delete those rows locally before `teardown_plan`.
- **Commit discipline**: commit only the intended pathspecs. Before any `./bin/travel` manual smoke,
  rebuild the release binary with `make build`.

## Source Facts Verified

- Harness:
  - `mod common; use common::Guard;`
  - `common::bin() -> &'static str`
  - `common::db_exec(sql: &str) -> Option<Rows>`
  - `common::seed_plan(plan, dest, version)` takes exactly 3 args and seeds `plan_destinations`.
  - `common::teardown_plan(plan, dest)` dynamically deletes all plan-keyed tables.
  - `common::is_credless(&stderr)` takes command stderr and should be checked after running a command.
  - `common::nanos()` returns a unique suffix.
- Schema:
  - `activities` has 24 columns. Seeded activity inserts in these tests supply
    `id`, `title`, `source`, `booking_required`, `is_fixed_time`, and `priority` explicitly.
  - `destination_pois` primary key is `(slug, poi_id)` and has `title`, `lat`, `lon`.
  - `day_route_segments` has `plan_id`, `destination`, `day_number`, `sort_order`, `from_place`,
    `to_place`, `mode`, `duration_min`, `notes`, `start_time`, and `source`.
  - `days` primary key is `(plan_id, destination, day_number)` and `date` is `NOT NULL`.
  - `operation_runs`, `plan_events`, and `plan_event_data` are the audit tables.

## Task A: Per-Day Map-Coverage WARN in `validate publish`

### Files

- Create: `rust/crates/travel-cli/tests/validate_publish_map_coverage.rs`
- Modify: `rust/crates/travel-cli/src/validate.rs`

### Interfaces

Consumes:

```rust
async fn has_map_path(
    conn: &libsql::Connection,
    plan_id: &str,
    destination: &str,
) -> Result<bool, String>;

async fn query_day_numbers(
    conn: &libsql::Connection,
    sql: &str,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<i64>, String>;
```

Produces:

```rust
async fn map_coverage_gaps(
    conn: &libsql::Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<i64>, String>;
```

Behavior:

- Called from `run_publish` immediately after the existing plan-wide `has_map_path` block.
- Pushes one `PublishIssue` per day:
  - `category: "map-coverage"`
  - `severity: PublishSeverity::Warn`
  - message: `day {day} has no geocoded stops and no route segments - its dashboard map will render empty. Link a real POI: set-activity-poi {day} <session> <poi_id> (or set-activity-poi --auto).`
- Read-only. No audit.

### A1. Write the Failing Integration Test

Add `rust/crates/travel-cli/tests/validate_publish_map_coverage.rs`:

```rust
//! Behavior locks for per-day map coverage warnings in `travel validate publish`.
//!
//! These tests hit shared Turso through the canonical `common::` harness. They
//! must be run serialized and in the background:
//! `cargo test -p travel-cli --test validate_publish_map_coverage -- --test-threads=1`.

mod common;
use common::{
    bin, db_exec, db_exec_teardown, is_credless, nanos, seed_plan, teardown_plan, Guard,
};

use std::process::Command;

const TODAY: &str = "2026-07-05";

fn run_publish(plan_id: &str, dest: &str) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(["validate", "publish", "--plan-id", plan_id, "--dest", dest])
        .env("TRAVEL_TODAY", TODAY)
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .unwrap_or_else(|e| panic!("run travel validate publish: {e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn seed_destination(dest: &str) {
    db_exec(&format!(
        "INSERT OR IGNORE INTO destination_config \
           (slug, display_name, ref_id, ref_path, timezone, currency, language, origin) \
         VALUES ('{dest}', 'ZZ Map Coverage Test', 'zz-ref', 'turso:destination-ref/zz', \
                 'Asia/Tokyo', 'JPY', 'ja', 'taiwan');"
    ))
    .expect("seed destination_config");
}

fn teardown_destination(dest: &str) {
    let _ = db_exec_teardown(&format!(
        "DELETE FROM destination_pois WHERE slug = '{dest}'; \
         DELETE FROM destination_config WHERE slug = '{dest}';"
    ));
}

fn seed_anchor(plan_id: &str, dest: &str, start: &str, end: &str, days: i64) {
    db_exec(&format!(
        "INSERT INTO date_anchors (plan_id, destination, start_date, end_date, days) \
         VALUES ('{plan_id}', '{dest}', '{start}', '{end}', {days});"
    ))
    .expect("seed date_anchors");
}

#[test]
fn null_poi_activity_day_warns_but_linked_and_zero_activity_days_do_not() {
    let tag = nanos();
    let plan_id = format!("test-mapcov-warn-{tag}");
    let dest = format!("mapcovwarn_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || {
            teardown_destination(&dest);
            teardown_plan(&plan_id, &dest);
        }
    });

    if db_exec("SELECT 1").is_none() {
        return;
    }

    seed_plan(&plan_id, &dest, 0);
    seed_destination(&dest);
    seed_anchor(&plan_id, &dest, "2026-02-01", "2026-02-03", 3);

    db_exec(&format!(
        "INSERT INTO destination_pois \
           (slug, poi_id, title, source_url, fetched_at, confidence, lat, lon) \
         VALUES ('{dest}', 'mapped_poi', 'Mapped POI', 'test', '2026-07-05', 'test', \
                 35.6812, 139.7671); \
         INSERT INTO days (plan_id, destination, day_number, date, day_type, theme, theme_zh) \
         VALUES ('{plan_id}', '{dest}', 1, '2026-02-01', 'arrival', 'Mapped', '有地圖'), \
                ('{plan_id}', '{dest}', 2, '2026-02-02', 'full', 'Unmapped', '無地圖'), \
                ('{plan_id}', '{dest}', 3, '2026-02-03', 'departure', 'Empty', '空白'); \
         INSERT INTO activities \
           (id, plan_id, destination, poi_id, day_number, session_type, sort_order, title, \
            booking_required, is_fixed_time, priority, source) \
         VALUES ('{plan_id}-mapped', '{plan_id}', '{dest}', 'mapped_poi', 1, 'morning', 0, \
                 'Mapped activity', 0, 0, 'want', 'confirmed'), \
                ('{plan_id}-unmapped', '{plan_id}', '{dest}', NULL, 2, 'morning', 0, \
                 'Unmapped real place', 0, 0, 'want', 'confirmed'); \
         INSERT INTO plan_map_snapshots (plan_id, snapshotted_at) \
         VALUES ('{plan_id}', datetime('now'));"
    ))
    .expect("seed map coverage case");

    let (_ok, stdout, stderr) = run_publish(&plan_id, &dest);
    if is_credless(&stderr) {
        return;
    }

    assert!(
        stdout.contains("[map-coverage] day 2 has no geocoded stops and no route segments"),
        "day 2 has an activity with poi_id=NULL and must warn. stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("[map-coverage] day 1 "),
        "day 1 has a linked geocoded POI and must not warn. stdout:\n{stdout}"
    );
    assert!(
        !stdout.contains("[map-coverage] day 3 "),
        "day 3 has zero activities and must not warn. stdout:\n{stdout}"
    );
}

#[test]
fn route_segment_day_does_not_warn_even_without_geocoded_activity_stops() {
    let tag = nanos();
    let plan_id = format!("test-mapcov-route-{tag}");
    let dest = format!("mapcovroute_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || {
            teardown_destination(&dest);
            teardown_plan(&plan_id, &dest);
        }
    });

    if db_exec("SELECT 1").is_none() {
        return;
    }

    seed_plan(&plan_id, &dest, 0);
    seed_destination(&dest);
    seed_anchor(&plan_id, &dest, "2026-02-01", "2026-02-01", 1);

    db_exec(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type, theme, theme_zh) \
         VALUES ('{plan_id}', '{dest}', 2, '2026-02-01', 'full', 'Route day', '路線日'); \
         INSERT INTO activities \
           (id, plan_id, destination, poi_id, day_number, session_type, sort_order, title, \
            booking_required, is_fixed_time, priority, source) \
         VALUES ('{plan_id}-unmapped', '{plan_id}', '{dest}', NULL, 2, 'morning', 0, \
                 'Unmapped real place', 0, 0, 'want', 'confirmed'); \
         INSERT INTO day_route_segments \
           (plan_id, destination, day_number, sort_order, from_place, to_place, mode, source) \
         VALUES ('{plan_id}', '{dest}', 2, 0, 'Station A', 'Station B', 'train', 'confirmed'); \
         INSERT INTO plan_map_snapshots (plan_id, snapshotted_at) \
         VALUES ('{plan_id}', datetime('now'));"
    ))
    .expect("seed route coverage case");

    let (_ok, stdout, stderr) = run_publish(&plan_id, &dest);
    if is_credless(&stderr) {
        return;
    }

    assert!(
        !stdout.contains("[map-coverage]"),
        "a day with route segments already has a dashboard route path and must not warn. stdout:\n{stdout}"
    );
}
```

Run RED in the background:

```bash
cd rust
cargo test -p travel-cli --test validate_publish_map_coverage -- --test-threads=1 \
  > /tmp/validate_publish_map_coverage.red.log 2>&1 & echo $!
```

Expected RED contains:

```text
test null_poi_activity_day_warns_but_linked_and_zero_activity_days_do_not
FAILED
day 2 has an activity with poi_id=NULL and must warn
```

The route negative may already pass because it only asserts absence. The required RED is the missing
day-2 `[map-coverage]` warning.

### A2. Minimal Implementation

Patch `rust/crates/travel-cli/src/validate.rs`.

Add this immediately after the existing plan-wide `has_map_path` block in `run_publish`:

```rust
    for day in map_coverage_gaps(&conn, plan_id, &destination).await? {
        issues.push(PublishIssue {
            category: "map-coverage".to_string(),
            severity: PublishSeverity::Warn,
            message: format!(
                "day {day} has no geocoded stops and no route segments - its dashboard map will render empty. Link a real POI: set-activity-poi {day} <session> <poi_id> (or set-activity-poi --auto)."
            ),
        });
    }
```

Add this helper near `has_map_path`:

```rust
async fn map_coverage_gaps(
    conn: &libsql::Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<i64>, String> {
    let sql = "WITH activity_days AS ( \
          SELECT day_number, COUNT(*) AS activity_count \
          FROM activities \
          WHERE plan_id = ?1 AND destination = ?2 \
          GROUP BY day_number \
        ), \
        mappable_days AS ( \
          SELECT a.day_number, COUNT(*) AS mappable_count \
          FROM activities a \
          JOIN destination_pois p \
            ON p.slug = a.destination AND p.poi_id = a.poi_id \
          WHERE a.plan_id = ?1 AND a.destination = ?2 \
            AND p.lat IS NOT NULL AND p.lon IS NOT NULL \
            AND TRIM(CAST(p.lat AS TEXT)) <> '' \
            AND TRIM(CAST(p.lon AS TEXT)) <> '' \
          GROUP BY a.day_number \
        ), \
        route_days AS ( \
          SELECT day_number, COUNT(*) AS route_count \
          FROM day_route_segments \
          WHERE plan_id = ?1 AND destination = ?2 \
          GROUP BY day_number \
        ) \
        SELECT ad.day_number \
        FROM activity_days ad \
        LEFT JOIN mappable_days md USING (day_number) \
        LEFT JOIN route_days rd USING (day_number) \
        WHERE COALESCE(md.mappable_count, 0) = 0 \
          AND COALESCE(rd.route_count, 0) = 0 \
        ORDER BY ad.day_number";
    query_day_numbers(conn, sql, plan_id, destination).await
}
```

The `mappable_days` CTE intentionally keeps the `has_map_path` POI predicate byte-identical, with only
`GROUP BY a.day_number` added.

### A3. Run GREEN, Then Commit

Run GREEN in the background:

```bash
cd rust
cargo test -p travel-cli --test validate_publish_map_coverage -- --test-threads=1 \
  > /tmp/validate_publish_map_coverage.green.log 2>&1 & echo $!
```

Expected GREEN:

```text
running 2 tests
test null_poi_activity_day_warns_but_linked_and_zero_activity_days_do_not
test route_segment_day_does_not_warn_even_without_geocoded_activity_stops
ok

test result: ok. 2 passed; 0 failed
```

Run the adjacent publish suite:

```bash
cd rust
cargo test -p travel-cli --test validate_publish -- --test-threads=1 \
  > /tmp/validate_publish.regression.log 2>&1 & echo $!
```

Expected:

```text
test result: ok.
```

Commit only these pathspecs:

```bash
git status --short
git diff -- rust/crates/travel-cli/src/validate.rs rust/crates/travel-cli/tests/validate_publish_map_coverage.rs
git add -- rust/crates/travel-cli/src/validate.rs rust/crates/travel-cli/tests/validate_publish_map_coverage.rs
git commit -m "Warn on per-day map coverage gaps" -- \
  rust/crates/travel-cli/src/validate.rs \
  rust/crates/travel-cli/tests/validate_publish_map_coverage.rs
```

Before any manual `./bin/travel validate publish <args>` smoke, rebuild the release binary:

```bash
make build
```

## Task B: `set-activity-poi --auto [--dest]`

### Files

- Create: `rust/crates/travel-cli/tests/set_activity_poi_auto.rs`
- Modify: `rust/crates/travel-cli/src/set_activity_poi.rs`
- Modify: `docs/reference/CLI.md`
- Modify: `CLAUDE.md`

### Interfaces

Consumes:

```rust
travel_db::repo::itinerary::poi_exists(
    conn: &Connection,
    destination: &str,
    poi_id: &str,
) -> Result<bool, String>;

travel_db::repo::itinerary::set_activity_poi(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    activity_id: &str,
    poi_id: &str,
    now: &str,
) -> Result<u64, String>;

crate::cascade::common::record_operation(
    conn: &Connection,
    plan_id: &str,
    command_type: &str,
    summary: &str,
    version_before: i64,
    version_after: i64,
    now_db: &str,
) -> Result<(), String>;
```

Produces in `set_activity_poi.rs`:

```rust
async fn execute_auto(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<AutoResult, String>;

async fn list_null_poi_activities(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<AutoActivity>, String>;

async fn list_destination_pois_for_auto(
    conn: &Connection,
    destination: &str,
) -> Result<Vec<AutoPoi>, String>;

fn strip_trailing_zh_gloss(title: &str) -> String;
fn is_trailing_gloss_char(c: char) -> bool;
fn resolve_auto_match(activity_title: &str, pois: &[AutoPoi]) -> AutoMatch;
```

New private structs/enums:

```rust
#[derive(Debug, Clone)]
struct AutoActivity {
    id: String,
    day: i64,
    session: String,
    sort_order: i64,
    title: String,
}

#[derive(Debug, Clone)]
struct AutoPoi {
    poi_id: String,
    title: String,
    geocoded: bool,
}

#[derive(Debug, Clone)]
struct AutoLinked {
    activity: AutoActivity,
    poi_id: String,
}

#[derive(Debug, Clone)]
struct AutoUnlinked {
    activity: AutoActivity,
    reason: String,
}

#[derive(Debug, Clone)]
struct AutoResult {
    linked: Vec<AutoLinked>,
    unlinked: Vec<AutoUnlinked>,
    version_before: Option<i64>,
    version_after: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AutoMatch {
    Link(String),
    Unlinked(String),
}
```

Stable scan order:

```sql
ORDER BY day_number, session_type, sort_order, id
```

Audit:

- If `linked.len() == 0`, do not write audit and do not bump `plans.version`.
- If `linked.len() > 0`, call `record_operation` exactly once with:
  - `command_type = "set-activity-poi"`
  - `summary = format!("auto-linked {} POI(s)", linked.len())`
  - `version_after = version_before + 1`

### ZH-Gloss Strip Rule

Use this exact Rust predicate:

```rust
fn is_trailing_gloss_char(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '\u{4E00}'..='\u{9FFF}' // CJK Unified Ideographs
                | '\u{3040}'..='\u{309F}' // Hiragana
                | '\u{30A0}'..='\u{30FF}' // Katakana
                | '\u{3000}'..='\u{303F}' // CJK symbols/punctuation, including ideographic space
                | '\u{FF00}'..='\u{FFEF}' // Fullwidth forms and punctuation
        )
}
```

Strip only a trailing run that contains at least one non-whitespace character in those Unicode ranges:

```rust
fn strip_trailing_zh_gloss(title: &str) -> String {
    let trimmed = title.trim_end();
    let mut cut = trimmed.len();
    let mut saw_script_or_fullwidth = false;

    for (idx, ch) in trimmed.char_indices().rev() {
        if is_trailing_gloss_char(ch) {
            if !ch.is_whitespace() {
                saw_script_or_fullwidth = true;
            }
            cut = idx;
            continue;
        }
        break;
    }

    if saw_script_or_fullwidth {
        trimmed[..cut].trim_end().to_string()
    } else {
        trimmed.to_string()
    }
}
```

Justification: the known authored glosses are appended Traditional-Chinese/Japanese script labels after
Romaji activity titles. CJK Unified Ideographs covers Traditional/Simplified kanji/hanzi in these
titles; Hiragana/Katakana covers Japanese kana; CJK symbols and fullwidth forms cover the punctuation
and fullwidth spacing that may trail such glosses. Hangul and CJK Extension blocks are intentionally
not included because they are not in the current data and widening the predicate is unnecessary.

Match rule:

1. Strip trailing gloss from the activity title, lowercase ASCII/Unicode with `to_lowercase()`.
2. Exact case-insensitive equality against `destination_pois.title`; link if exactly one geocoded POI
   matches.
3. Else substring either direction, case-insensitive; link if exactly one geocoded POI matches.
4. If zero matches: unlinked `no POI match`.
5. If matches exist but zero geocoded matches: unlinked `matched but POI ungeocoded`.
6. If more than one geocoded match: unlinked `ambiguous POI match`.
7. No leading-token rule.

### B1. Write the Failing Integration Test

Add `rust/crates/travel-cli/tests/set_activity_poi_auto.rs`:

```rust
//! Integration tests for `travel set-activity-poi --auto`.
//!
//! These tests use shared Turso. Run serialized and in the background:
//! `cargo test -p travel-cli --test set_activity_poi_auto -- --test-threads=1`.

mod common;
use common::{
    bin, db_exec, db_exec_teardown, is_credless, nanos, seed_plan, teardown_plan, Guard,
};

use std::process::Command;

fn run_cmd(plan_id: &str, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(args)
        .env("TRAVEL_PLAN_ID", plan_id)
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn seed_destination(dest: &str) {
    db_exec(&format!(
        "INSERT OR IGNORE INTO destination_config \
           (slug, display_name, ref_id, ref_path, timezone, currency, language, origin) \
         VALUES ('{dest}', 'ZZ Auto POI Test', 'zz-ref', 'turso:destination-ref/zz', \
                 'Asia/Tokyo', 'JPY', 'ja', 'taiwan');"
    ))
    .expect("seed destination_config");
}

fn teardown_destination(dest: &str) {
    let _ = db_exec_teardown(&format!(
        "DELETE FROM destination_pois WHERE slug = '{dest}'; \
         DELETE FROM destination_config WHERE slug = '{dest}';"
    ));
}

fn scalar(sql: &str) -> Option<String> {
    db_exec(sql)?.scalar()
}

fn count(sql: &str) -> Option<i64> {
    scalar(sql).and_then(|s| s.parse::<i64>().ok())
}

fn seed_days(plan_id: &str, dest: &str) {
    db_exec(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type, theme, theme_zh) \
         VALUES ('{plan_id}', '{dest}', 1, '2026-02-01', 'full', 'Explore', '探索');"
    ))
    .expect("seed day");
}

#[test]
fn auto_links_exact_and_gloss_substring_but_leaves_ambiguous_no_match_and_ungeocoded() {
    let tag = nanos();
    let plan_id = format!("test-actpoi-auto-{tag}");
    let dest = format!("actpoiauto_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || {
            teardown_destination(&dest);
            teardown_plan(&plan_id, &dest);
        }
    });

    if db_exec("SELECT 1").is_none() {
        return;
    }

    seed_plan(&plan_id, &dest, 10);
    seed_destination(&dest);
    seed_days(&plan_id, &dest);

    db_exec(&format!(
        "INSERT INTO destination_pois (slug, poi_id, title, lat, lon) VALUES \
           ('{dest}', 'kuromon_market', 'Kuromon Ichiba Market', 34.6650, 135.5063), \
           ('{dest}', 'dotonbori', 'Dotonbori Canal', 34.6687, 135.5013), \
           ('{dest}', 'namba_parks', 'Namba Parks', 34.6617, 135.5017), \
           ('{dest}', 'namba_yasaka', 'Namba Yasaka Shrine', 34.6623, 135.4960), \
           ('{dest}', 'tsutenkaku', 'Tsutenkaku Tower', NULL, NULL); \
         INSERT INTO activities \
           (id, plan_id, destination, poi_id, day_number, session_type, sort_order, title, \
            booking_required, is_fixed_time, priority, source) \
         VALUES \
           ('{plan_id}-exact', '{plan_id}', '{dest}', NULL, 1, 'morning', 0, \
            'Kuromon Ichiba Market 黑門市場', 0, 0, 'want', 'confirmed'), \
           ('{plan_id}-substr', '{plan_id}', '{dest}', NULL, 1, 'morning', 1, \
            'Dotonbori 道頓堀', 0, 0, 'want', 'confirmed'), \
           ('{plan_id}-ambig', '{plan_id}', '{dest}', NULL, 1, 'afternoon', 0, \
            'Namba', 0, 0, 'want', 'confirmed'), \
           ('{plan_id}-nomatch', '{plan_id}', '{dest}', NULL, 1, 'afternoon', 1, \
            'Shinsaibashi-suji Shopping 心齋橋筋商店街', 0, 0, 'want', 'confirmed'), \
           ('{plan_id}-ungeocoded', '{plan_id}', '{dest}', NULL, 1, 'evening', 0, \
            'Tsutenkaku Tower 通天閣', 0, 0, 'want', 'confirmed');"
    ))
    .expect("seed auto case");

    let (ok, stdout, stderr) = run_cmd(&plan_id, &["set-activity-poi", "--auto", "--dest", &dest]);
    if is_credless(&stderr) {
        return;
    }

    assert!(ok, "--auto should succeed. stdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(stdout.contains("linked 2"), "expected two links. stdout:\n{stdout}");
    assert!(stdout.contains("Kuromon Ichiba Market 黑門市場"), "exact+gloss row missing. stdout:\n{stdout}");
    assert!(stdout.contains("Dotonbori 道頓堀"), "substring+gloss row missing. stdout:\n{stdout}");
    assert!(stdout.contains("unlinked 3"), "expected three manual rows. stdout:\n{stdout}");
    assert!(stdout.contains("ambiguous POI match"), "ambiguous reason missing. stdout:\n{stdout}");
    assert!(stdout.contains("no POI match"), "no-match reason missing. stdout:\n{stdout}");
    assert!(
        stdout.contains("matched but POI ungeocoded"),
        "ungeocoded reason missing. stdout:\n{stdout}"
    );

    assert_eq!(
        scalar(&format!(
            "SELECT COALESCE(poi_id, 'NULL') AS poi_id FROM activities \
             WHERE plan_id = '{plan_id}' AND id = '{plan_id}-exact'"
        )),
        Some("kuromon_market".to_string())
    );
    assert_eq!(
        scalar(&format!(
            "SELECT COALESCE(poi_id, 'NULL') AS poi_id FROM activities \
             WHERE plan_id = '{plan_id}' AND id = '{plan_id}-substr'"
        )),
        Some("dotonbori".to_string())
    );
    for id in ["ambig", "nomatch", "ungeocoded"] {
        assert_eq!(
            scalar(&format!(
                "SELECT COALESCE(poi_id, 'NULL') AS poi_id FROM activities \
                 WHERE plan_id = '{plan_id}' AND id = '{plan_id}-{id}'"
            )),
            Some("NULL".to_string()),
            "{id} must remain unlinked"
        );
    }

    assert_eq!(
        count(&format!(
            "SELECT COUNT(*) AS n FROM operation_runs \
             WHERE plan_id = '{plan_id}' AND command_type = 'set-activity-poi' \
               AND command_summary = 'auto-linked 2 POI(s)' AND status = 'completed'"
        )),
        Some(1),
        "auto mode must write exactly one completed operation_runs row"
    );
    assert_eq!(
        scalar(&format!("SELECT version FROM plans WHERE plan_id = '{plan_id}'")),
        Some("11".to_string()),
        "auto mode must bump version exactly once"
    );
}

#[test]
fn auto_second_run_is_noop_when_no_null_poi_activities_remain() {
    let tag = nanos();
    let plan_id = format!("test-actpoi-auto-idem-{tag}");
    let dest = format!("actpoiautoidem_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || {
            teardown_destination(&dest);
            teardown_plan(&plan_id, &dest);
        }
    });

    if db_exec("SELECT 1").is_none() {
        return;
    }

    seed_plan(&plan_id, &dest, 7);
    seed_destination(&dest);
    seed_days(&plan_id, &dest);

    db_exec(&format!(
        "INSERT INTO destination_pois (slug, poi_id, title, lat, lon) VALUES \
           ('{dest}', 'osaka_castle', 'Osaka Castle', 34.6873, 135.5262), \
           ('{dest}', 'umeda_sky', 'Umeda Sky Building', 34.7054, 135.4900); \
         INSERT INTO activities \
           (id, plan_id, destination, poi_id, day_number, session_type, sort_order, title, \
            booking_required, is_fixed_time, priority, source) \
         VALUES \
           ('{plan_id}-castle', '{plan_id}', '{dest}', NULL, 1, 'morning', 0, \
            'Osaka Castle 大阪城', 0, 0, 'want', 'confirmed'), \
           ('{plan_id}-umeda', '{plan_id}', '{dest}', NULL, 1, 'afternoon', 0, \
            'Umeda Sky Building 梅田スカイビル', 0, 0, 'want', 'confirmed');"
    ))
    .expect("seed idempotence case");

    let (ok1, stdout1, stderr1) = run_cmd(&plan_id, &["set-activity-poi", "--auto", "--dest", &dest]);
    if is_credless(&stderr1) {
        return;
    }
    assert!(ok1, "first --auto should link both rows. stdout:\n{stdout1}\nstderr:\n{stderr1}");
    assert!(stdout1.contains("linked 2"), "first run should link 2. stdout:\n{stdout1}");

    let (ok2, stdout2, stderr2) = run_cmd(&plan_id, &["set-activity-poi", "--auto", "--dest", &dest]);
    assert!(ok2, "second --auto should be a noop. stdout:\n{stdout2}\nstderr:\n{stderr2}");
    assert!(
        stdout2.contains("nothing to link"),
        "second run should report nothing to link. stdout:\n{stdout2}"
    );
    assert_eq!(
        count(&format!(
            "SELECT COUNT(*) AS n FROM operation_runs \
             WHERE plan_id = '{plan_id}' AND command_type = 'set-activity-poi' \
               AND command_summary = 'auto-linked 2 POI(s)' AND status = 'completed'"
        )),
        Some(1),
        "second noop must not write another operation_runs row"
    );
    assert_eq!(
        scalar(&format!("SELECT version FROM plans WHERE plan_id = '{plan_id}'")),
        Some("8".to_string()),
        "second noop must not bump version"
    );
}

#[test]
fn auto_rejects_positionals() {
    let tag = nanos();
    let plan_id = format!("test-actpoi-auto-args-{tag}");
    let dest = format!("actpoiautoargs_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || {
            teardown_destination(&dest);
            teardown_plan(&plan_id, &dest);
        }
    });

    if db_exec("SELECT 1").is_none() {
        return;
    }

    seed_plan(&plan_id, &dest, 0);
    seed_destination(&dest);

    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &["set-activity-poi", "--auto", "1", "morning", "foo", "--dest", &dest],
    );
    if is_credless(&stderr) {
        return;
    }

    assert!(!ok, "--auto with positionals must exit non-zero. stdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(
        stderr.contains("--auto cannot be combined with <day> <session> <poi_id>"),
        "expected fail-loud usage error. stderr:\n{stderr}"
    );
}
```

Run RED in the background:

```bash
cd rust
cargo test -p travel-cli --test set_activity_poi_auto -- --test-threads=1 \
  > /tmp/set_activity_poi_auto.red.log 2>&1 & echo $!
```

Expected RED contains:

```text
unknown argument: --auto
auto_links_exact_and_gloss_substring_but_leaves_ambiguous_no_match_and_ungeocoded
auto_second_run_is_noop_when_no_null_poi_activities_remain
auto_rejects_positionals
FAILED
```

### B2. Add Parser Support and Parser Unit Tests

Patch `ParsedArgs`:

```rust
#[derive(Default, Debug)]
struct ParsedArgs {
    auto: bool,
    day: i64,
    session: String,
    poi_id: String,
    match_substr: Option<String>,
    dest: Option<String>,
}
```

Patch `parse_args`:

```rust
            "--auto" => {
                p.auto = true;
                i += 1;
            }
```

Replace the positional validation tail with:

```rust
    if p.auto {
        if !positional.is_empty() {
            return Err(
                "--auto cannot be combined with <day> <session> <poi_id>; use either --auto or a single manual link"
                    .to_string(),
            );
        }
        if p.match_substr.is_some() {
            return Err("--auto cannot be combined with --match".to_string());
        }
        return Ok(p);
    }

    if positional.len() < 3 {
        return Err(usage_error());
    }
```

Update `usage_error`:

```rust
fn usage_error() -> String {
    "Usage: set-activity-poi <day> <session> <poi_id> [--match \"<title substring>\"] [--dest <slug>]
       set-activity-poi --auto [--dest <slug>]
Example: set-activity-poi 2 morning shuri_castle --match \"Shurijo\"
Example: set-activity-poi --auto --dest osaka_kyoto_2026"
        .to_string()
}
```

Add parser unit tests in the existing `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn parse_args_accepts_auto_with_dest() {
        let p = parse_args(&[
            "--auto".to_string(),
            "--dest".to_string(),
            "osaka_kyoto_2026".to_string(),
        ])
        .unwrap();
        assert!(p.auto);
        assert_eq!(p.dest.as_deref(), Some("osaka_kyoto_2026"));
    }

    #[test]
    fn parse_args_rejects_auto_with_positionals() {
        let err = parse_args(&[
            "--auto".to_string(),
            "1".to_string(),
            "morning".to_string(),
            "foo".to_string(),
        ])
        .unwrap_err();
        assert!(err.contains("--auto cannot be combined with <day> <session> <poi_id>"));
    }
```

Run parser/unit test:

```bash
cd rust
cargo test -p travel-cli set_activity_poi::tests -- --test-threads=1 \
  > /tmp/set_activity_poi_parser.green.log 2>&1 & echo $!
```

Expected:

```text
test result: ok.
```

Do not commit yet; the integration test should still fail until `execute_auto` exists.

### B3. Implement Auto Matching Helpers

Add the structs/enums from the Interfaces section near `ParsedArgs`.

Add:

```rust
fn is_trailing_gloss_char(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '\u{4E00}'..='\u{9FFF}'
                | '\u{3040}'..='\u{309F}'
                | '\u{30A0}'..='\u{30FF}'
                | '\u{3000}'..='\u{303F}'
                | '\u{FF00}'..='\u{FFEF}'
        )
}

fn strip_trailing_zh_gloss(title: &str) -> String {
    let trimmed = title.trim_end();
    let mut cut = trimmed.len();
    let mut saw_script_or_fullwidth = false;

    for (idx, ch) in trimmed.char_indices().rev() {
        if is_trailing_gloss_char(ch) {
            if !ch.is_whitespace() {
                saw_script_or_fullwidth = true;
            }
            cut = idx;
            continue;
        }
        break;
    }

    if saw_script_or_fullwidth {
        trimmed[..cut].trim_end().to_string()
    } else {
        trimmed.to_string()
    }
}

fn resolve_auto_match(activity_title: &str, pois: &[AutoPoi]) -> AutoMatch {
    let normalized = strip_trailing_zh_gloss(activity_title).to_lowercase();
    let exact: Vec<&AutoPoi> = pois
        .iter()
        .filter(|p| p.title.to_lowercase() == normalized)
        .collect();
    let matches = if exact.is_empty() {
        pois.iter()
            .filter(|p| {
                let title = p.title.to_lowercase();
                !normalized.is_empty()
                    && !title.is_empty()
                    && (normalized.contains(&title) || title.contains(&normalized))
            })
            .collect::<Vec<&AutoPoi>>()
    } else {
        exact
    };

    if matches.is_empty() {
        return AutoMatch::Unlinked("no POI match".to_string());
    }

    let geocoded: Vec<&AutoPoi> = matches.iter().copied().filter(|p| p.geocoded).collect();
    match geocoded.len() {
        0 => AutoMatch::Unlinked("matched but POI ungeocoded".to_string()),
        1 => AutoMatch::Link(geocoded[0].poi_id.clone()),
        _ => AutoMatch::Unlinked("ambiguous POI match".to_string()),
    }
}
```

Add unit tests:

```rust
    #[test]
    fn strip_trailing_zh_gloss_removes_only_trailing_gloss() {
        assert_eq!(strip_trailing_zh_gloss("Dotonbori 道頓堀"), "Dotonbori");
        assert_eq!(
            strip_trailing_zh_gloss("Kuromon Ichiba Market 黑門市場"),
            "Kuromon Ichiba Market"
        );
        assert_eq!(strip_trailing_zh_gloss("Umeda Sky Building 梅田スカイビル"), "Umeda Sky Building");
        assert_eq!(strip_trailing_zh_gloss("Namba Parks"), "Namba Parks");
    }

    #[test]
    fn resolve_auto_match_exact_substring_ambiguous_and_ungeocoded() {
        let pois = vec![
            AutoPoi { poi_id: "kuromon".to_string(), title: "Kuromon Ichiba Market".to_string(), geocoded: true },
            AutoPoi { poi_id: "dotonbori".to_string(), title: "Dotonbori Canal".to_string(), geocoded: true },
            AutoPoi { poi_id: "namba1".to_string(), title: "Namba Parks".to_string(), geocoded: true },
            AutoPoi { poi_id: "namba2".to_string(), title: "Namba Yasaka Shrine".to_string(), geocoded: true },
            AutoPoi { poi_id: "tower".to_string(), title: "Tsutenkaku Tower".to_string(), geocoded: false },
        ];
        assert_eq!(
            resolve_auto_match("Kuromon Ichiba Market 黑門市場", &pois),
            AutoMatch::Link("kuromon".to_string())
        );
        assert_eq!(
            resolve_auto_match("Dotonbori 道頓堀", &pois),
            AutoMatch::Link("dotonbori".to_string())
        );
        assert_eq!(
            resolve_auto_match("Namba", &pois),
            AutoMatch::Unlinked("ambiguous POI match".to_string())
        );
        assert_eq!(
            resolve_auto_match("No Such Place", &pois),
            AutoMatch::Unlinked("no POI match".to_string())
        );
        assert_eq!(
            resolve_auto_match("Tsutenkaku Tower 通天閣", &pois),
            AutoMatch::Unlinked("matched but POI ungeocoded".to_string())
        );
    }
```

Run unit tests:

```bash
cd rust
cargo test -p travel-cli set_activity_poi::tests -- --test-threads=1 \
  > /tmp/set_activity_poi_helpers.green.log 2>&1 & echo $!
```

Expected:

```text
test result: ok.
```

### B4. Implement Reads, `execute_auto`, and Output

Patch `run` after destination resolution:

```rust
    if parsed.auto {
        match execute_auto(&conn, &plan_id, &destination).await {
            Ok(result) => {
                print_auto_result(&destination, &result);
                return Ok(());
            }
            Err(e) => {
                eprintln!("Error: set-activity-poi --auto failed: {e}");
                std::process::exit(1);
            }
        }
    }
```

Add the read helpers:

```rust
async fn list_null_poi_activities(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<AutoActivity>, String> {
    let mut rows = conn
        .query(
            "SELECT id, day_number, session_type, sort_order, title \
             FROM activities \
             WHERE plan_id = ?1 AND destination = ?2 AND poi_id IS NULL \
             ORDER BY day_number, session_type, sort_order, id",
            libsql::params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("NULL-poi activities query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("NULL-poi activities row read failed: {e}"))?
    {
        out.push(AutoActivity {
            id: row.get(0).map_err(|e| format!("activity id col read failed: {e}"))?,
            day: row.get(1).map_err(|e| format!("activity day col read failed: {e}"))?,
            session: row.get(2).map_err(|e| format!("activity session col read failed: {e}"))?,
            sort_order: row.get(3).map_err(|e| format!("activity sort_order col read failed: {e}"))?,
            title: row.get(4).map_err(|e| format!("activity title col read failed: {e}"))?,
        });
    }
    Ok(out)
}

async fn list_destination_pois_for_auto(
    conn: &Connection,
    destination: &str,
) -> Result<Vec<AutoPoi>, String> {
    let mut rows = conn
        .query(
            "SELECT poi_id, COALESCE(title, ''), \
                    CASE WHEN lat IS NOT NULL AND lon IS NOT NULL \
                           AND TRIM(CAST(lat AS TEXT)) <> '' \
                           AND TRIM(CAST(lon AS TEXT)) <> '' \
                         THEN 1 ELSE 0 END AS geocoded \
             FROM destination_pois \
             WHERE slug = ?1 \
             ORDER BY poi_id",
            libsql::params![destination.to_string()],
        )
        .await
        .map_err(|e| format!("destination_pois auto query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("destination_pois auto row read failed: {e}"))?
    {
        let geocoded: i64 = row.get(2).unwrap_or(0);
        out.push(AutoPoi {
            poi_id: row.get(0).map_err(|e| format!("poi_id col read failed: {e}"))?,
            title: row.get(1).unwrap_or_default(),
            geocoded: geocoded == 1,
        });
    }
    Ok(out)
}
```

Add `execute_auto`:

```rust
async fn execute_auto(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<AutoResult, String> {
    let activities = list_null_poi_activities(conn, plan_id, destination).await?;
    if activities.is_empty() {
        return Ok(AutoResult {
            linked: Vec::new(),
            unlinked: Vec::new(),
            version_before: None,
            version_after: None,
        });
    }

    let pois = list_destination_pois_for_auto(conn, destination).await?;
    let mut linked = Vec::new();
    let mut unlinked = Vec::new();

    for activity in activities {
        match resolve_auto_match(&activity.title, &pois) {
            AutoMatch::Link(poi_id) => linked.push(AutoLinked { activity, poi_id }),
            AutoMatch::Unlinked(reason) => unlinked.push(AutoUnlinked { activity, reason }),
        }
    }

    if linked.is_empty() {
        return Ok(AutoResult {
            linked,
            unlinked,
            version_before: None,
            version_after: None,
        });
    }

    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();
    let version_before = read_version(conn, plan_id).await?;
    let version_after = version_before + 1;
    let mut touched_days: Vec<i64> = Vec::new();

    let mut dest_process_so =
        next_dest_process_sort_order(conn, plan_id, destination, "process_5_daily_itinerary").await?;
    let mut timeline_so = next_timeline_sort_order(conn, plan_id).await?;

    for link in &linked {
        let affected = itinerary::set_activity_poi(
            conn,
            plan_id,
            destination,
            link.activity.day,
            &link.activity.session,
            &link.activity.id,
            &link.poi_id,
            &now_db,
        )
        .await?;
        if affected != 1 {
            return Err(format!(
                "activities UPDATE affected {affected} rows (expected 1) for id={}",
                link.activity.id
            ));
        }

        if !touched_days.contains(&link.activity.day) {
            touch_day(conn, plan_id, destination, link.activity.day).await?;
            touched_days.push(link.activity.day);
        }

        let kv: Vec<(&str, String)> = vec![
            ("day_number", link.activity.day.to_string()),
            ("session", link.activity.session.clone()),
            ("activity_id", link.activity.id.clone()),
            ("title", link.activity.title.clone()),
            ("poi_id", link.poi_id.clone()),
            ("mode", "auto".to_string()),
        ];
        insert_event(
            conn,
            plan_id,
            "dest_process",
            destination,
            "process_5_daily_itinerary",
            dest_process_so,
            "activity_poi_linked",
            &now_iso,
        )
        .await?;
        insert_kv(
            conn,
            plan_id,
            "dest_process",
            destination,
            "process_5_daily_itinerary",
            dest_process_so,
            &kv,
        )
        .await?;
        dest_process_so += 1;

        insert_event(
            conn,
            plan_id,
            "timeline",
            "",
            "process_5_daily_itinerary",
            timeline_so,
            "activity_poi_linked",
            &now_iso,
        )
        .await?;
        insert_kv(
            conn,
            plan_id,
            "timeline",
            "",
            "process_5_daily_itinerary",
            timeline_so,
            &kv,
        )
        .await?;
        timeline_so += 1;
    }

    crate::cascade::common::record_operation(
        conn,
        plan_id,
        "set-activity-poi",
        &format!("auto-linked {} POI(s)", linked.len()),
        version_before,
        version_after,
        &now_db,
    )
    .await?;

    Ok(AutoResult {
        linked,
        unlinked,
        version_before: Some(version_before),
        version_after: Some(version_after),
    })
}
```

Add `print_auto_result`:

```rust
fn print_auto_result(destination: &str, result: &AutoResult) {
    println!("\n📍 set-activity-poi --auto ({destination})");
    if result.linked.is_empty() && result.unlinked.is_empty() {
        println!("nothing to link");
        return;
    }

    println!("✅ linked {}:", result.linked.len());
    for link in &result.linked {
        println!(
            "   D{} {:<9} \"{}\" -> {}",
            link.activity.day, link.activity.session, link.activity.title, link.poi_id
        );
    }

    if !result.unlinked.is_empty() {
        println!(
            "⚠ {} unlinked (link manually with set-activity-poi <day> <session> <poi_id> --match \"<title substring>\"):",
            result.unlinked.len()
        );
        for item in &result.unlinked {
            println!(
                "   D{} {:<9} \"{}\" - {}",
                item.activity.day, item.activity.session, item.activity.title, item.reason
            );
        }
    }

    if let (Some(before), Some(after)) = (result.version_before, result.version_after) {
        println!(
            "Version {before} -> {after} (auto-linked {} POI(s)).",
            result.linked.len()
        );
    }
}
```

Notes:

- This uses `->` and `-` in new output for ASCII consistency. Existing emoji output remains consistent
  with the command's style.
- `sort_order` is retained on `AutoActivity` even if only used for scan order/debugging; do not expose it
  in output.
- If libsql transaction support is straightforward in this codebase, wrap the UPDATE/event/audit block in
  one transaction. If not, keep the fail-loud sequential writes above; the command is re-runnable because
  already-linked rows are skipped on the next run.

### B5. Run GREEN, Then Existing Regression Tests

Run the new integration test in the background:

```bash
cd rust
cargo test -p travel-cli --test set_activity_poi_auto -- --test-threads=1 \
  > /tmp/set_activity_poi_auto.green.log 2>&1 & echo $!
```

Expected:

```text
running 3 tests
test auto_links_exact_and_gloss_substring_but_leaves_ambiguous_no_match_and_ungeocoded
test auto_second_run_is_noop_when_no_null_poi_activities_remain
test auto_rejects_positionals
ok

test result: ok. 3 passed; 0 failed
```

Run existing manual-link regression tests in the background:

```bash
cd rust
cargo test -p travel-cli --test set_activity_poi -- --test-threads=1 \
  > /tmp/set_activity_poi.regression.log 2>&1 & echo $!
```

Expected:

```text
test result: ok. 3 passed; 0 failed
```

Run parser/helper unit tests:

```bash
cd rust
cargo test -p travel-cli set_activity_poi::tests -- --test-threads=1 \
  > /tmp/set_activity_poi.unit.log 2>&1 & echo $!
```

Expected:

```text
test result: ok.
```

### B6. Update Plain-Text Docs

Patch `docs/reference/CLI.md` near the existing itinerary mutation commands:

```text
./bin/travel set-activity-poi <day> <session> <poi_id> [--match "<title substring>"] [--dest slug]    # link one activity to a destination_pois row; durable map/ticket POI FK; writes audit triad.
./bin/travel set-activity-poi --auto [--dest slug]    # batch-link NULL-poi activities to exactly-one geocoded POI by exact/title-substring match after stripping trailing CJK/kana/fullwidth gloss; never guesses; one operation_runs row for the batch; unambiguous misses stay manual.
```

Patch the `CLAUDE.md` Skill Decision Tree row around empty maps / POI linking:

```text
"empty dashboard map" / "link itinerary POIs"  → ./bin/travel set-activity-poi --auto [--dest slug] first; then manually link any reported misses with ./bin/travel set-activity-poi <day> <session> <poi_id> --match "<title substring>"
```

Do not add JSON docs or fixtures.

### B7. Commit

Commit only these pathspecs:

```bash
git status --short
git diff -- \
  rust/crates/travel-cli/src/set_activity_poi.rs \
  rust/crates/travel-cli/tests/set_activity_poi_auto.rs \
  docs/reference/CLI.md \
  CLAUDE.md
git add -- \
  rust/crates/travel-cli/src/set_activity_poi.rs \
  rust/crates/travel-cli/tests/set_activity_poi_auto.rs \
  docs/reference/CLI.md \
  CLAUDE.md
git commit -m "Auto-link itinerary activities to POIs" -- \
  rust/crates/travel-cli/src/set_activity_poi.rs \
  rust/crates/travel-cli/tests/set_activity_poi_auto.rs \
  docs/reference/CLI.md \
  CLAUDE.md
```

Before any manual `./bin/travel set-activity-poi --auto <args>` smoke, rebuild the release binary:

```bash
make build
```

Do not run auto-link smoke against a real plan unless the operator explicitly wants that mutation. Prefer
the integration test's throwaway plan coverage.

## Final Verification

Run both new integration suites in the background:

```bash
cd rust
cargo test -p travel-cli --test validate_publish_map_coverage -- --test-threads=1 \
  > /tmp/final_validate_publish_map_coverage.log 2>&1 & echo $!
cargo test -p travel-cli --test set_activity_poi_auto -- --test-threads=1 \
  > /tmp/final_set_activity_poi_auto.log 2>&1 & echo $!
```

Run adjacent regressions:

```bash
cd rust
cargo test -p travel-cli --test validate_publish -- --test-threads=1 \
  > /tmp/final_validate_publish.log 2>&1 & echo $!
cargo test -p travel-cli --test set_activity_poi -- --test-threads=1 \
  > /tmp/final_set_activity_poi.log 2>&1 & echo $!
cargo test -p travel-cli set_activity_poi::tests -- --test-threads=1 \
  > /tmp/final_set_activity_poi_unit.log 2>&1 & echo $!
```

Expected for each log:

```text
test result: ok.
```

Run a compile check:

```bash
cd rust
cargo build -p travel-cli
```

Before any release-binary smoke:

```bash
make build
```

## Implementation Notes and Traps

- Do not drive Part A's per-day query from the inner POI join alone. That drops exactly the
  `poi_id=NULL` days that need warning. Start from `activity_days`.
- Do not make map coverage a blocker. It is a warning.
- Do not warn on zero-activity days. Content-depth owns thin/empty days.
- Do not reject duplicate POI use in Part B. `activities.poi_id` is nullable with no unique constraint.
- Do not link ungeocoded POIs in auto mode. They do not fix dashboard map rendering.
- Do not add the dropped leading-token matcher.
- Do not call `record_operation` per linked activity. Auto mode is one logical mutation.
- Do not add local files or JSON fixtures. Seed Turso rows directly in integration tests and render/assert
  plain text.
