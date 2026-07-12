# 伴手禮推薦 feature Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add omiyage (souvenir) recommendation + purchase-location capability: two normalized reference tables, a slug-keyed `add-omiyage` write command + `query-omiyage` view (like add-transit — no --plan-id, no audit), reusing existing `destination_pois` as sellers, with real provenance and validate-data invariants.

**Architecture:** 5 tasks: (1) schema + db_migrate for 2 tables; (2) `travel-db::repo::omiyage` DAL (existence checks + **one atomic transactional writer** + query read); (3) `add-omiyage` command; (4) `query-omiyage` command; (5) `validate data` invariants on a **dedicated path**. Each = one testable deliverable.

**Tech Stack:** Rust (travel-cli command layer + travel-db repo layer), libsql/Turso, real-Turso integration tests on `tests/common/mod.rs`.

## Global Constraints

- **No hardcode / Turso-only / no JSON in RDB / agent-first plain text / fail loud** — per the spec `docs/superpowers/specs/2026-07-12-omiyage-recommendation-feature.md` (authoritative — read it).
- **Slug-keyed GLOBAL reference data, NO audit triad** — `add-omiyage` writes NO plan_events/operation_runs/plans.version, even with `$TRAVEL_PLAN_ID` set. Mirrors `add-transit` (`add_transit.rs:1-15,30-75`).
- **db_migrate.rs creates the tables (idempotent `CREATE TABLE IF NOT EXISTS`); schema.sql mirrors the DDL.** Not both create. Wrapper is **`exec_lenient(&conn, r#"..."#).await;`** (`db_migrate.rs:1408`) — **NOT** `migrate_exec`.
- **Exact write contract (spec L64–71):** single transactional repo writer (Task 2) — `BEGIN` → validate slug + same-slug POI → read `(slug,item_id)` → absent: insert full item then upsert location; present: DO NOT write item (item-bundle flags optional; **only SUPPLIED flags must MATCH**, else fail) → upsert location only → `COMMIT` only on expected affected-row counts; any error / unexpected count → `ROLLBACK` (no partial write).
- **confidence ∈ {verified, reviewed}** on both tables (no `estimate`); source_url required + **must start with `http://` or `https://`** (prefix check, not bare substring); fetched_at auto-stamped (`chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)` like `add_transit.rs:37`).
- **POI existence — reuse, do not invent:** call `destination_ref::poi_coords_exists(conn, slug, poi_id) -> Result<bool, String>` (`destination_ref.rs:268`; SQL = `SELECT 1 FROM destination_pois WHERE slug=?1 AND poi_id=?2`). **Do NOT invent `poi_exists_for_slug`.**
- **destination_pois seed columns — only columns that exist in `scripts/schema.sql`:** `slug, poi_id, title, area, nearest_station, duration_min, booking_required, booking_url, cost_estimate, notes, hours, address, source_url, fetched_at, confidence, lat, lon`. **There is NO `source` column** — use `source_url` (see `set_poi_coords.rs:30-31` / `add_transit_derive_routes.rs:65-67`).
- **INSERT OR REPLACE affected-row count:** `add_transit` asserts `affected != 1` (`add_transit.rs:56`). For omiyage location upsert, **do not blindly copy `== 1`** — libsql may report **1 or 2** for REPLACE. Implementer **must verify actual libsql affected-row count** on insert-vs-replace once; **prefer `affected >= 1`** (fail only on `0`) unless live verification proves a strict single value. Plain `INSERT` for new items still expects exactly `1`.
- **CRITICAL test teardown** — these are non-plan-keyed global rows; `common::teardown_plan` will NOT clean them. Every test that writes omiyage/pois/config rows: **arm `common::Guard::new(closure)` FIRST (right after ids are bound), BEFORE seeding** — precedent `set_poi_coords.rs:62-68` / `add_transit_derive_routes.rs:51-56`. Closure deletes in dependency order: `destination_omiyage_locations` → `destination_omiyage_items` → `destination_pois` → `destination_config` (via `common::db_exec_teardown`). Optional defensive pre-clean after Guard arm; never a trailing-only teardown.
- **Rows API (tests/common/mod.rs:56-68):** `scalar() -> Option<String>` and `column() -> Vec<String>` — **NOT** `it[0][0]`. Assert like `rows.scalar().as_deref() == Some("1")`.
- **Serialized real-Turso tests in the BACKGROUND** (`--test-threads=1`); a foreground timeout SIGTERMs mid-run and the Guard Drop never fires (leaks a prod row).
- **Pipeline** — Grok 4.5 implements task-by-task; Claude reviews every line + corroborates vs source + verifies serialized. Codex reviews plan/test-plan when quota returns; otherwise Grok 4.5 assists reviews but Claude remains the quality gate. Commit explicit pathspecs only.

---

## File Structure

- `rust/crates/travel-cli/src/db_migrate.rs` — add 2 `CREATE TABLE IF NOT EXISTS` via `exec_lenient` (near destination_transit at :1408–1424). (Task 1)
- `scripts/schema.sql` — mirror the 2 table DDLs. (Task 1)
- `rust/crates/travel-db/src/repo/omiyage.rs` — NEW repo: `config_slug_exists`, **`write_item_and_location` (ONE atomic transactional writer)**, `read_item` (for tests/match visibility), `query_omiyage` (join read). Reuses `destination_ref::poi_coords_exists` for POI checks (call from writer or re-export path — do not duplicate SQL). (Task 2)
- `rust/crates/travel-db/src/repo/mod.rs` — `pub mod omiyage;`. (Task 2)
- `rust/crates/travel-cli/src/add_omiyage.rs` — NEW command. (Task 3)
- `rust/crates/travel-cli/src/query_omiyage.rs` — NEW command. (Task 4)
- `rust/crates/travel-cli/src/main.rs` — `mod add_omiyage;` + `mod query_omiyage;` near other mods; dispatch arms like add-transit; optionally list both in `print_usage()`. (Tasks 3+4)
- `rust/crates/travel-cli/src/validate.rs` — **dedicated** omiyage validate path (NOT the empty-table checklist at :1005). (Task 5)
- `rust/crates/travel-cli/tests/omiyage.rs` — NEW behavior-lock test file (all tasks add cases here).
- `docs/reference/CLI.md` — document both commands (near add-transit / set-poi-coords). (Task 4)

---

## Task 1 (commit 1) — schema + migration for the 2 tables

**Files:**
- Modify: `rust/crates/travel-cli/src/db_migrate.rs` (add 2 CREATE near :1408), `scripts/schema.sql`
- Test: `rust/crates/travel-cli/tests/omiyage.rs` (new)

- [ ] **Step 1: Write the failing test**

Create `rust/crates/travel-cli/tests/omiyage.rs`. Assert the 2 tables exist with the right columns after `db migrate`.

```rust
mod common;
use common::{bin, db_exec, is_credless, nanos, db_exec_teardown, Guard, seed_plan, teardown_plan};

use std::process::Command;

fn run(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(args)
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .expect("run travel");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Dependency-order teardown for omiyage + seeded ref rows (non-plan-keyed).
fn omi_teardown(slug: &str) {
    let s = slug.replace('\'', "''");
    let _ = db_exec_teardown(&format!(
        "DELETE FROM destination_omiyage_locations WHERE slug='{s}'; \
         DELETE FROM destination_omiyage_items WHERE slug='{s}'; \
         DELETE FROM destination_pois WHERE slug='{s}'; \
         DELETE FROM destination_config WHERE slug='{s}';"
    ));
}

/// Seed destination_config + one destination_pois seller.
/// Columns verified against scripts/schema.sql — NO invented `source` column.
fn seed_dest_and_poi(slug: &str, poi: &str) -> bool {
    if db_exec("SELECT 1").is_none() {
        return false;
    }
    let s = slug.replace('\'', "''");
    let p = poi.replace('\'', "''");
    db_exec(&format!(
        "INSERT INTO destination_config (slug, display_name, timezone, currency, origin) \
           VALUES ('{s}', 'Omi Test', 'Asia/Tokyo', 'JPY', 'taiwan'); \
         INSERT INTO destination_pois \
           (slug, poi_id, title, area, nearest_station, hours, address, source_url, fetched_at, confidence) \
           VALUES ('{s}', '{p}', 'Test Depachika', 'namba', 'Namba', '10:00-20:00', NULL, \
                   'https://example.com/poi', '2026-07-12T00:00:00Z', 'verified');"
    ))
    .is_some()
}

#[test]
fn omiyage_tables_exist_after_migrate() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless");
        return;
    };
    let _ = run(&["db", "migrate"]);
    // both tables queryable with the specified columns (empty is fine)
    assert!(
        db_exec(
            "SELECT slug,item_id,name,category,notes,source_url,fetched_at,confidence \
             FROM destination_omiyage_items LIMIT 0"
        )
        .is_some(),
        "destination_omiyage_items must have the 8 columns"
    );
    assert!(
        db_exec(
            "SELECT slug,item_id,poi_id,purchase_note,source_url,fetched_at,confidence \
             FROM destination_omiyage_locations LIMIT 0"
        )
        .is_some(),
        "destination_omiyage_locations must have the 7 columns"
    );
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cd rust && cargo test -p travel-cli --test omiyage omiyage_tables_exist -- --test-threads=1 --nocapture
```
Expected: FAIL — `no such table: destination_omiyage_items` (or SELECT fails).

- [ ] **Step 3: Add the CREATE statements in db_migrate.rs**

Near `destination_transit` (`db_migrate.rs:1408–1424`), add two **`exec_lenient(&conn, r#"..."#).await;`** calls (match the surrounding call style exactly — **NOT** `migrate_exec`). All columns TEXT:

```rust
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
```

- [ ] **Step 4: Mirror in schema.sql**

Add both `CREATE TABLE destination_omiyage_items (...)` / `destination_omiyage_locations (...)` to `scripts/schema.sql` in the destination-reference section (near `destination_transit` ~:368), same column order, `PRIMARY KEY` as above. All TEXT; `notes`/`purchase_note` optional in practice (may be NULL); remaining required enforced by write-time validation + validate data.

- [ ] **Step 5: Build + run to verify pass**

```bash
cd rust && cargo build -p travel-cli && cargo test -p travel-cli --test omiyage omiyage_tables_exist -- --test-threads=1 --nocapture
```
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add rust/crates/travel-cli/src/db_migrate.rs scripts/schema.sql rust/crates/travel-cli/tests/omiyage.rs
git commit -F - <<'EOF'
feat(omiyage): schema + migration for items + locations tables

Two normalized slug-keyed reference tables: destination_omiyage_items
(slug,item_id,name,category,notes,source_url,fetched_at,confidence) and
destination_omiyage_locations (slug,item_id,poi_id,purchase_note,source_url,
fetched_at,confidence). db_migrate creates via exec_lenient (idempotent);
schema.sql mirrors.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Task 2 (commit 2) — `travel-db::repo::omiyage` DAL (atomic writer + query)

**Files:**
- Create: `rust/crates/travel-db/src/repo/omiyage.rs`
- Modify: `rust/crates/travel-db/src/repo/mod.rs` (`pub mod omiyage;`)
- Test: **first RED for end-to-end behavior is Task 3** (repo has no CLI surface). Task 2's gate is: module compiles, is exported, and is ready for Task 3/4 to lock. Optionally add a compile-only / unit-level check; **do not invent a fake CLI just for Task 2.**

**Interfaces (Produces — the command layer consumes these):**

- `async fn config_slug_exists(conn, slug) -> Result<bool, String>` — `SELECT 1 FROM destination_config WHERE slug=?1`.
- **POI existence: REUSE `destination_ref::poi_coords_exists(conn, slug, poi_id)`** (`destination_ref.rs:268`). Call it from `write_item_and_location` (or from the CLI before write — prefer **inside** the writer so atomic path owns validation). **Do NOT invent `poi_exists_for_slug`.**
- `async fn read_item(conn, slug, item_id) -> Result<Option<OmiyageItem>, String>` — for match visibility / tests.  
  `struct OmiyageItem { name: String, category: String, notes: Option<String>, source_url: String, confidence: String }` — **fetched_at excluded from match**.
- **`async fn write_item_and_location(...) -> Result<WriteOutcome, String>`** — **ONE transactional writer** (pick (a) from the review; cleaner + testable). Signature sketch:

```rust
pub struct ItemFlags<'a> {
    pub name: Option<&'a str>,
    pub category: Option<&'a str>,
    pub notes: Option<&'a str>,
    pub source_url: Option<&'a str>,
    pub confidence: Option<&'a str>,
}

pub struct LocationInput<'a> {
    pub poi_id: &'a str,
    pub purchase_note: Option<&'a str>,
    pub source_url: &'a str,
    pub confidence: &'a str,
}

pub enum WriteOutcome {
    CreatedItemAndLocation,
    UpsertedLocationOnly,
}

/// Atomic write per spec L64–71.
/// BEGIN → config_slug_exists + poi_coords_exists → read_item →
///   None: require full item bundle (name/category/source_url/confidence all present + non-blank)
///         → plain INSERT item (expect affected==1) → INSERT OR REPLACE location
///   Some: do NOT write item; for each SUPPLIED ItemFlags field, require exact match
///         (omit/None = do not compare that field) → INSERT OR REPLACE location only
/// → on any Err or unexpected affected count: ROLLBACK and return Err
/// → else COMMIT.
/// Location affected-row: verify libsql INSERT OR REPLACE count; assert `>= 1` (safer) unless live proof says otherwise.
pub async fn write_item_and_location(
    conn: &libsql::Connection,
    slug: &str,
    item_id: &str,
    item: ItemFlags<'_>,
    location: LocationInput<'_>,
    fetched_at: &str,
) -> Result<WriteOutcome, String>;
```

Transaction pattern (mirror `turso-util` migrate `begin`/`finish` at `migrate.rs:204-223` — same conn, raw SQL):

```rust
conn.execute("BEGIN", libsql::params![]).await.map_err(...)?;
let result = async {
    // validate + insert/upsert...
    Ok(outcome)
}.await;
match result {
    Ok(o) => {
        conn.execute("COMMIT", libsql::params![]).await.map_err(...)?;
        Ok(o)
    }
    Err(e) => {
        let _ = conn.execute("ROLLBACK", libsql::params![]).await;
        Err(e)
    }
}
```

- `async fn insert_item` / `async fn upsert_location` — **private helpers** used only inside the transaction (not public CLI surface). Plain `INSERT` for item (NOT REPLACE). `INSERT OR REPLACE` for location on `(slug,item_id,poi_id)`.
- `async fn query_omiyage(conn, slug) -> Result<Vec<OmiyageRow>, String>` — join items ⨝ locations ⨝ destination_pois, ordered category, name, item_id, poi title, poi_id.

```sql
SELECT i.item_id, i.name, i.category, i.notes, i.source_url, i.confidence, i.fetched_at,
       l.poi_id, p.title, p.area, p.nearest_station, p.address, p.hours,
       l.purchase_note, l.source_url, l.confidence, l.fetched_at
FROM destination_omiyage_items i
JOIN destination_omiyage_locations l ON l.slug=i.slug AND l.item_id=i.item_id
LEFT JOIN destination_pois p ON p.slug=l.slug AND p.poi_id=l.poi_id
WHERE i.slug=?1
ORDER BY i.category, i.name, i.item_id, p.title, l.poi_id
```

`struct OmiyageRow { item_id, name, category, item_notes: Option, item_source_url, item_confidence, item_fetched_at, poi_id, poi_title: Option, area: Option, station: Option, address: Option, hours: Option, purchase_note: Option, loc_source_url, loc_confidence, loc_fetched_at }`. LEFT JOIN so orphans surface as NULL poi fields; command fails loud on corrupt rows.

**Partial-item-bundle match rule (spec L69–70, CLI-GAP):** when item already exists, only compare **SUPPLIED** flags. Example: second seller with only `--buy-at` + location provenance → no item fields supplied → skip all match checks. If `--name "X"` is supplied and stored name is `"Y"` → fail, no location write. Unsupplied fields are **not** treated as blank mismatches.

- [ ] **Step 1: RED boundary for Task 2**

There is no CLI yet. **First RED that exercises this repo is Task 3's `add_omiyage_*` tests.** For Task 2 alone:

1. Create the module with the public API above (stubs that `todo!()` or real impl).
2. `cargo build -p travel-db` must succeed once implemented.
3. Do **not** claim behavior-lock green until Task 3/4.

If implementing for real in this task (recommended): implement fully so Task 3 only wires CLI; Task 2 commit still has no end-to-end test lock.

- [ ] **Step 2: Implement `repo/omiyage.rs`**

Model existence SELECTs on `destination_ref`; transaction + private insert/upsert; `query_omiyage` as above. Call `crate::repo::destination_ref::poi_coords_exists` (or `super::destination_ref::poi_coords_exists`) from the writer.

- [ ] **Step 3: Register the module** in `repo/mod.rs`: `pub mod omiyage;`.

- [ ] **Step 4: Build**

```bash
cd rust && cargo build -p travel-db -p travel-cli
```
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/travel-db/src/repo/omiyage.rs rust/crates/travel-db/src/repo/mod.rs
git commit -F - <<'EOF'
feat(omiyage): travel-db repo::omiyage DAL (atomic write + join query)

config_slug_exists, write_item_and_location (BEGIN/COMMIT/ROLLBACK: insert-if-absent
or verify SUPPLIED-flag match then location upsert), query_omiyage (items ⨝ locations
⨝ destination_pois, deterministic order, NULL poi for orphan detection). Reuses
destination_ref::poi_coords_exists — no invented poi_exists helper.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Task 3 (commit 3) — `add-omiyage` command

**Files:**
- Create: `rust/crates/travel-cli/src/add_omiyage.rs`
- Modify: `rust/crates/travel-cli/src/main.rs`:
  - add `mod add_omiyage;` near `mod add_transit;` (~line 31)
  - dispatch arm: `[cmd, rest @ ..] if cmd == "add-omiyage" => { add_omiyage::run(rest).await }` (same shape as add-transit at main.rs:229–231)
  - optionally add a line under VALIDATE / CHECKS in `print_usage()` (~852)
- Test: `rust/crates/travel-cli/tests/omiyage.rs`

**Interfaces:**
- Consumes: `repo::omiyage::{config_slug_exists, write_item_and_location, read_item}` + (indirectly) `destination_ref::poi_coords_exists` inside the writer.
- Produces:  
  `travel add-omiyage <slug> <item_id> --buy-at <poi_id> --location-source-url <url> --location-confidence verified|reviewed [--name] [--category] [--item-source-url] [--item-confidence] [--notes] [--purchase-note]`

- [ ] **Step 1: Write the failing tests** (full seed→invoke→assert; Guard FIRST)

Append to `tests/omiyage.rs`. Shared helpers from Task 1 (`run`, `omi_teardown`, `seed_dest_and_poi`).

```rust
#[test]
fn add_omiyage_creates_item_and_location() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = nanos();
    let slug = format!("omi_dest_{n}");
    let poi = format!("omi_poi_{n}");
    // Guard FIRST — right after ids bound, BEFORE seed (set_poi_coords.rs:62-68)
    let _g = Guard::new({
        let s = slug.clone();
        move || omi_teardown(&s)
    });
    omi_teardown(&slug); // defensive pre-clean
    if !seed_dest_and_poi(&slug, &poi) {
        eprintln!("credless on seed");
        return;
    }

    let (ok, _o, e) = run(&[
        "add-omiyage",
        &slug,
        "tokyo_banana",
        "--name",
        "Tokyo Banana",
        "--category",
        "和菓子",
        "--buy-at",
        &poi,
        "--item-source-url",
        "https://www.tokyobanana.jp/",
        "--item-confidence",
        "verified",
        "--location-source-url",
        "https://example.com/floor",
        "--location-confidence",
        "verified",
    ]);
    assert!(ok, "add-omiyage should succeed; err={e}");

    let it = db_exec(&format!(
        "SELECT COUNT(*) FROM destination_omiyage_items \
         WHERE slug='{slug}' AND item_id='tokyo_banana'"
    ))
    .expect("count items");
    assert_eq!(it.scalar().as_deref(), Some("1"));

    let lo = db_exec(&format!(
        "SELECT COUNT(*) FROM destination_omiyage_locations \
         WHERE slug='{slug}' AND item_id='tokyo_banana' AND poi_id='{poi}'"
    ))
    .expect("count locations");
    assert_eq!(lo.scalar().as_deref(), Some("1"));
}

#[test]
fn add_omiyage_second_seller_keeps_item_unchanged() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = nanos();
    let slug = format!("omi_dest2_{n}");
    let poi1 = format!("omi_poi_a_{n}");
    let poi2 = format!("omi_poi_b_{n}");
    let _g = Guard::new({
        let s = slug.clone();
        move || omi_teardown(&s)
    });
    omi_teardown(&slug);
    if !seed_dest_and_poi(&slug, &poi1) {
        return;
    }
    // second POI same slug (real columns only)
    let s = slug.replace('\'', "''");
    let p2 = poi2.replace('\'', "''");
    if db_exec(&format!(
        "INSERT INTO destination_pois \
           (slug, poi_id, title, area, nearest_station, source_url, fetched_at, confidence) \
           VALUES ('{s}', '{p2}', 'Second Seller', 'umeda', 'Umeda', \
                   'https://example.com/poi2', '2026-07-12T00:00:00Z', 'verified');"
    ))
    .is_none()
    {
        return;
    }

    // first seller + full item bundle
    let (ok1, _, e1) = run(&[
        "add-omiyage", &slug, "item_x",
        "--name", "Item X", "--category", "名產",
        "--buy-at", &poi1,
        "--item-source-url", "https://example.com/item",
        "--item-confidence", "verified",
        "--location-source-url", "https://example.com/loc1",
        "--location-confidence", "verified",
        "--notes", "keep-me",
    ]);
    assert!(ok1, "first add; err={e1}");

    let before = db_exec(&format!(
        "SELECT name||'|'||category||'|'||COALESCE(notes,'')||'|'||source_url||'|'||confidence \
         FROM destination_omiyage_items WHERE slug='{slug}' AND item_id='item_x'"
    ))
    .and_then(|r| r.scalar())
    .expect("item snapshot");

    // second seller: item bundle OMITTED
    let (ok2, _, e2) = run(&[
        "add-omiyage", &slug, "item_x",
        "--buy-at", &poi2,
        "--location-source-url", "https://example.com/loc2",
        "--location-confidence", "reviewed",
    ]);
    assert!(ok2, "second seller; err={e2}");

    let after = db_exec(&format!(
        "SELECT name||'|'||category||'|'||COALESCE(notes,'')||'|'||source_url||'|'||confidence \
         FROM destination_omiyage_items WHERE slug='{slug}' AND item_id='item_x'"
    ))
    .and_then(|r| r.scalar())
    .expect("item after");
    assert_eq!(before, after, "item row must be byte-identical after 2nd seller");

    let nloc = db_exec(&format!(
        "SELECT COUNT(*) FROM destination_omiyage_locations WHERE slug='{slug}' AND item_id='item_x'"
    ))
    .expect("loc count");
    assert_eq!(nloc.scalar().as_deref(), Some("2"));

    // sub-case: mismatched SUPPLIED bundle → fail, no 3rd location, item still unchanged
    let (ok_bad, _, e_bad) = run(&[
        "add-omiyage", &slug, "item_x",
        "--name", "WRONG NAME",
        "--buy-at", &poi1,
        "--location-source-url", "https://example.com/loc3",
        "--location-confidence", "verified",
    ]);
    assert!(!ok_bad, "mismatched name must fail; stderr={e_bad}");
    let after_bad = db_exec(&format!(
        "SELECT name||'|'||category||'|'||COALESCE(notes,'')||'|'||source_url||'|'||confidence \
         FROM destination_omiyage_items WHERE slug='{slug}' AND item_id='item_x'"
    ))
    .and_then(|r| r.scalar())
    .expect("item after mismatch");
    assert_eq!(before, after_bad);
    let nloc2 = db_exec(&format!(
        "SELECT COUNT(*) FROM destination_omiyage_locations WHERE slug='{slug}' AND item_id='item_x'"
    ))
    .expect("loc count after mismatch");
    assert_eq!(nloc2.scalar().as_deref(), Some("2"));
}

#[test]
fn add_omiyage_same_seller_re_add_refreshes_fetched_at() {
    // Spec L147 / L71: same (slug,item_id,poi_id) re-add → still 1 location row, fetched_at refreshed.
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = nanos();
    let slug = format!("omi_re_{n}");
    let poi = format!("omi_poi_re_{n}");
    let _g = Guard::new({
        let s = slug.clone();
        move || omi_teardown(&s)
    });
    omi_teardown(&slug);
    if !seed_dest_and_poi(&slug, &poi) {
        return;
    }

    let (ok1, _, e1) = run(&[
        "add-omiyage", &slug, "re_item",
        "--name", "Re Item", "--category", "藥妝",
        "--buy-at", &poi,
        "--item-source-url", "https://example.com/re-item",
        "--item-confidence", "verified",
        "--location-source-url", "https://example.com/re-loc",
        "--location-confidence", "verified",
    ]);
    assert!(ok1, "first; err={e1}");

    let fa1 = db_exec(&format!(
        "SELECT fetched_at FROM destination_omiyage_locations \
         WHERE slug='{slug}' AND item_id='re_item' AND poi_id='{poi}'"
    ))
    .and_then(|r| r.scalar())
    .expect("fetched_at1");

    std::thread::sleep(std::time::Duration::from_millis(20));

    // re-add same seller (item bundle optional; omit)
    let (ok2, _, e2) = run(&[
        "add-omiyage", &slug, "re_item",
        "--buy-at", &poi,
        "--location-source-url", "https://example.com/re-loc-v2",
        "--location-confidence", "reviewed",
    ]);
    assert!(ok2, "re-add; err={e2}");

    let nloc = db_exec(&format!(
        "SELECT COUNT(*) FROM destination_omiyage_locations \
         WHERE slug='{slug}' AND item_id='re_item' AND poi_id='{poi}'"
    ))
    .expect("count");
    assert_eq!(nloc.scalar().as_deref(), Some("1"));

    let fa2 = db_exec(&format!(
        "SELECT fetched_at FROM destination_omiyage_locations \
         WHERE slug='{slug}' AND item_id='re_item' AND poi_id='{poi}'"
    ))
    .and_then(|r| r.scalar())
    .expect("fetched_at2");
    assert_ne!(fa1, fa2, "fetched_at must refresh on same-seller re-add");

    let conf = db_exec(&format!(
        "SELECT confidence FROM destination_omiyage_locations \
         WHERE slug='{slug}' AND item_id='re_item' AND poi_id='{poi}'"
    ))
    .and_then(|r| r.scalar());
    assert_eq!(conf.as_deref(), Some("reviewed"));
}

/// Atomic / zero-partial: every fail path leaves 0 rows on both tables.
/// (Spec L148: failure create → 0 item + 0 location. True mid-tx ROLLBACK is
/// implemented in write_item_and_location; CLI-level proof is zero residual rows
/// on all validation failures below + the matrix.)
#[test]
fn add_omiyage_fail_loud_matrix() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = nanos();
    let slug = format!("omi_fail_{n}");
    let poi = format!("omi_poi_fail_{n}");
    let other_slug = format!("omi_other_{n}");
    let _g = Guard::new({
        let (s1, s2) = (slug.clone(), other_slug.clone());
        move || {
            omi_teardown(&s1);
            omi_teardown(&s2);
        }
    });
    omi_teardown(&slug);
    omi_teardown(&other_slug);
    if !seed_dest_and_poi(&slug, &poi) {
        return;
    }
    // wrong-slug POI: POI exists under other_slug only
    if !seed_dest_and_poi(&other_slug, "poi_elsewhere") {
        return;
    }

    let assert_zero = |label: &str| {
        let it = db_exec(&format!(
            "SELECT COUNT(*) FROM destination_omiyage_items WHERE slug='{slug}'"
        ))
        .expect("items");
        let lo = db_exec(&format!(
            "SELECT COUNT(*) FROM destination_omiyage_locations WHERE slug='{slug}'"
        ))
        .expect("locs");
        assert_eq!(it.scalar().as_deref(), Some("0"), "{label}: items");
        assert_eq!(lo.scalar().as_deref(), Some("0"), "{label}: locations");
    };

    // unknown dest
    {
        let (ok, _, e) = run(&[
            "add-omiyage", "no_such_dest_xyz", "i1",
            "--name", "N", "--category", "C",
            "--buy-at", &poi,
            "--item-source-url", "https://example.com/i",
            "--item-confidence", "verified",
            "--location-source-url", "https://example.com/l",
            "--location-confidence", "verified",
        ]);
        assert!(!ok, "unknown dest; stderr={e}");
    }

    // unknown POI (same slug, poi missing)
    {
        let (ok, _, e) = run(&[
            "add-omiyage", &slug, "i1",
            "--name", "N", "--category", "C",
            "--buy-at", "no_such_poi",
            "--item-source-url", "https://example.com/i",
            "--item-confidence", "verified",
            "--location-source-url", "https://example.com/l",
            "--location-confidence", "verified",
        ]);
        assert!(!ok, "unknown poi; stderr={e}");
        assert_zero("unknown poi");
    }

    // wrong-slug POI (poi exists only on other_slug)
    {
        let (ok, _, e) = run(&[
            "add-omiyage", &slug, "i1",
            "--name", "N", "--category", "C",
            "--buy-at", "poi_elsewhere",
            "--item-source-url", "https://example.com/i",
            "--item-confidence", "verified",
            "--location-source-url", "https://example.com/l",
            "--location-confidence", "verified",
        ]);
        assert!(!ok, "wrong-slug poi; stderr={e}");
        assert_zero("wrong-slug poi");
    }

    // blank name on new item
    {
        let (ok, _, e) = run(&[
            "add-omiyage", &slug, "i1",
            "--name", "   ", "--category", "C",
            "--buy-at", &poi,
            "--item-source-url", "https://example.com/i",
            "--item-confidence", "verified",
            "--location-source-url", "https://example.com/l",
            "--location-confidence", "verified",
        ]);
        assert!(!ok, "blank name; stderr={e}");
        assert_zero("blank name");
    }

    // blank category on new item
    {
        let (ok, _, e) = run(&[
            "add-omiyage", &slug, "i1",
            "--name", "N", "--category", "",
            "--buy-at", &poi,
            "--item-source-url", "https://example.com/i",
            "--item-confidence", "verified",
            "--location-source-url", "https://example.com/l",
            "--location-confidence", "verified",
        ]);
        assert!(!ok, "blank category; stderr={e}");
        assert_zero("blank category");
    }

    // missing --location-source-url
    {
        let (ok, _, e) = run(&[
            "add-omiyage", &slug, "i1",
            "--name", "N", "--category", "C",
            "--buy-at", &poi,
            "--item-source-url", "https://example.com/i",
            "--item-confidence", "verified",
            "--location-confidence", "verified",
        ]);
        assert!(!ok, "missing loc url; stderr={e}");
        assert_zero("missing loc url");
    }

    // bad --location-confidence 'guess'
    {
        let (ok, _, e) = run(&[
            "add-omiyage", &slug, "i1",
            "--name", "N", "--category", "C",
            "--buy-at", &poi,
            "--item-source-url", "https://example.com/i",
            "--item-confidence", "verified",
            "--location-source-url", "https://example.com/l",
            "--location-confidence", "guess",
        ]);
        assert!(!ok, "bad confidence; stderr={e}");
        assert_zero("bad confidence");
    }

    // non-http url (must start with http:// or https://)
    {
        let (ok, _, e) = run(&[
            "add-omiyage", &slug, "i1",
            "--name", "N", "--category", "C",
            "--buy-at", &poi,
            "--item-source-url", "ftp://example.com/i",
            "--item-confidence", "verified",
            "--location-source-url", "https://example.com/l",
            "--location-confidence", "verified",
        ]);
        assert!(!ok, "non-http url; stderr={e}");
        assert_zero("non-http url");
    }

    // missing flag value
    {
        let (ok, _, e) = run(&[
            "add-omiyage", &slug, "i1",
            "--name", "N", "--category", "C",
            "--buy-at", &poi,
            "--item-source-url", "https://example.com/i",
            "--item-confidence", "verified",
            "--location-source-url", // no value
        ]);
        assert!(!ok, "missing flag value; stderr={e}");
        assert_zero("missing flag value");
    }

    // excess positional
    {
        let (ok, _, e) = run(&[
            "add-omiyage", &slug, "i1", "extra_pos",
            "--name", "N", "--category", "C",
            "--buy-at", &poi,
            "--item-source-url", "https://example.com/i",
            "--item-confidence", "verified",
            "--location-source-url", "https://example.com/l",
            "--location-confidence", "verified",
        ]);
        assert!(!ok, "excess positional; stderr={e}");
        assert_zero("excess positional");
    }

    // unknown flag
    {
        let (ok, _, e) = run(&[
            "add-omiyage", &slug, "i1",
            "--name", "N", "--category", "C",
            "--buy-at", &poi,
            "--item-source-url", "https://example.com/i",
            "--item-confidence", "verified",
            "--location-source-url", "https://example.com/l",
            "--location-confidence", "verified",
            "--not-a-real-flag", "x",
        ]);
        assert!(!ok, "unknown flag; stderr={e}");
        assert_zero("unknown flag");
    }

    // --plan-id (friendly reject; explicit parse arm — not generic reject_unknown_flags alone)
    {
        let (ok, _, e) = run(&[
            "add-omiyage", &slug, "i1",
            "--name", "N", "--category", "C",
            "--buy-at", &poi,
            "--item-source-url", "https://example.com/i",
            "--item-confidence", "verified",
            "--location-source-url", "https://example.com/l",
            "--location-confidence", "verified",
            "--plan-id", "some-plan",
        ]);
        assert!(!ok, "--plan-id; stderr={e}");
        assert!(
            e.contains("plan-id") || e.contains("--plan-id") || e.contains("no --plan-id"),
            "friendly --plan-id message; stderr={e}"
        );
        assert_zero("--plan-id");
    }

    // new-item missing bundle (no --name/--category/--item-source-url/--item-confidence)
    {
        let (ok, _, e) = run(&[
            "add-omiyage", &slug, "i1",
            "--buy-at", &poi,
            "--location-source-url", "https://example.com/l",
            "--location-confidence", "verified",
        ]);
        assert!(!ok, "new-item missing bundle; stderr={e}");
        assert_zero("new-item missing bundle");
    }
}

#[test]
fn add_omiyage_no_audit_even_with_travel_plan_id() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = nanos();
    let plan = format!("zz-omi-plan-{n}");
    let slug = format!("omi_audit_{n}");
    let poi = format!("omi_poi_audit_{n}");

    // Combined Guard: plan-keyed + omiyage/ref rows
    let _g = Guard::new({
        let (plan, slug) = (plan.clone(), slug.clone());
        move || {
            omi_teardown(&slug);
            teardown_plan(&plan, &slug);
        }
    });
    omi_teardown(&slug);
    teardown_plan(&plan, &slug);
    seed_plan(&plan, &slug, 7);
    if !seed_dest_and_poi(&slug, &poi) {
        return;
    }

    let ver_before = db_exec(&format!(
        "SELECT version FROM plans WHERE plan_id='{plan}'"
    ))
    .and_then(|r| r.scalar())
    .expect("version before");
    let runs_before = db_exec(&format!(
        "SELECT COUNT(*) FROM operation_runs WHERE plan_id='{plan}'"
    ))
    .and_then(|r| r.scalar())
    .unwrap_or_else(|| "0".into());
    let events_before = db_exec(&format!(
        "SELECT COUNT(*) FROM plan_events WHERE plan_id='{plan}'"
    ))
    .and_then(|r| r.scalar())
    .unwrap_or_else(|| "0".into());

    let out = Command::new(bin())
        .args([
            "add-omiyage",
            &slug,
            "audit_item",
            "--name",
            "Audit Item",
            "--category",
            "名產",
            "--buy-at",
            &poi,
            "--item-source-url",
            "https://example.com/a",
            "--item-confidence",
            "verified",
            "--location-source-url",
            "https://example.com/al",
            "--location-confidence",
            "verified",
        ])
        .env("TRAVEL_PLAN_ID", &plan)
        .output()
        .expect("run with TRAVEL_PLAN_ID");
    assert!(
        out.status.success(),
        "add should succeed with TRAVEL_PLAN_ID set; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let ver_after = db_exec(&format!(
        "SELECT version FROM plans WHERE plan_id='{plan}'"
    ))
    .and_then(|r| r.scalar())
    .expect("version after");
    assert_eq!(ver_before, ver_after, "plans.version must not bump");

    let runs_after = db_exec(&format!(
        "SELECT COUNT(*) FROM operation_runs WHERE plan_id='{plan}'"
    ))
    .and_then(|r| r.scalar())
    .unwrap_or_else(|| "0".into());
    assert_eq!(runs_before, runs_after, "no operation_runs row");

    let events_after = db_exec(&format!(
        "SELECT COUNT(*) FROM plan_events WHERE plan_id='{plan}'"
    ))
    .and_then(|r| r.scalar())
    .unwrap_or_else(|| "0".into());
    assert_eq!(events_before, events_after, "no plan_events row");
}
```

- [ ] **Step 2: Run to verify fail** (command doesn't exist → unknown command / non-zero).

```bash
cd rust && cargo test -p travel-cli --test omiyage add_omiyage_creates -- --test-threads=1 --nocapture
```

- [ ] **Step 3: Implement `add_omiyage.rs`**

Structure like `add_transit.rs`:

1. `--help` / `-h` guard → print usage, Ok.
2. **Parse** (owned flags: `--buy-at`, `--location-source-url`, `--location-confidence`, `--name`, `--category`, `--item-source-url`, `--item-confidence`, `--notes`, `--purchase-note`):
   - **Explicit early arm for `--plan-id`** with a friendly message (mirror `add_transit.rs:121-127` — e.g. *"add-omiyage is global/slug-keyed reference data and takes no --plan-id"*). The generic `reject_unknown_flags` / catch-all alone will **not** produce a friendly "no --plan-id here" message.
   - Catch-all `other if other.starts_with("--")` → unknown argument.
   - Exactly 2 positionals: `<slug> <item_id>`; excess → fail; blank identity → fail.
   - confidence ∈ {verified, reviewed} only.
   - URLs: **must start with `http://` or `https://`** (prefix).
   - `--buy-at` / `--location-source-url` / `--location-confidence` always required.
3. `connect_write` → stamp `fetched_at` → call **`repo::omiyage::write_item_and_location`** (single call; CLI does **not** issue BEGIN/COMMIT itself — Task 2 owns the transaction).
4. Plain-text success output (item, category, seller POI, confidence, whether new item or location-only). **NO audit triad.**
5. Register wiring in `main.rs` (mod + arm). Optionally `print_usage()`.

**Location affected-row note for implementer:** inside the repo writer, after location upsert, verify libsql's returned count once (insert vs replace). **Prefer `if affected < 1 { Err(...) }`** rather than blindly `!= 1` (REPLACE may report 1 or 2).

- [ ] **Step 4: Build + run tests serialized**

```bash
cd rust && cargo build -p travel-cli && cargo test -p travel-cli --test omiyage -- --test-threads=1 --nocapture
```
Expected: Task 1+3 tests green (Task 4/5 not yet).

- [ ] **Step 5: Commit**

```bash
git add rust/crates/travel-cli/src/add_omiyage.rs rust/crates/travel-cli/src/main.rs rust/crates/travel-cli/tests/omiyage.rs
git commit -F - <<'EOF'
feat(omiyage): add-omiyage command (slug-keyed, atomic, no audit)

CLI orchestrates parse + write_item_and_location. Explicit --plan-id reject,
HTTP(S) URL prefix check, full fail-loud matrix + second-seller + re-add
fetched_at + no-audit-with-TRAVEL_PLAN_ID locks.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Task 4 (commit 4) — `query-omiyage` command + CLI.md

**Files:**
- Create: `rust/crates/travel-cli/src/query_omiyage.rs`
- Modify: `main.rs` — `mod query_omiyage;` + arm `[cmd, rest @ ..] if cmd == "query-omiyage" => query_omiyage::run(rest).await`; optionally `print_usage()` under VIEWS next to `query-destination-ref`
- Modify: `docs/reference/CLI.md` (near set-poi-coords / add-transit ~:117–118)
- Test: `rust/crates/travel-cli/tests/omiyage.rs`

**Interfaces:** Consumes `repo::omiyage::{config_slug_exists, query_omiyage}`. Produces `travel query-omiyage --slug <slug>`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn query_omiyage_round_trip_grouped_with_dual_provenance() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = nanos();
    let slug = format!("omi_q_{n}");
    let poi = format!("omi_poi_q_{n}");
    let _g = Guard::new({
        let s = slug.clone();
        move || omi_teardown(&s)
    });
    omi_teardown(&slug);
    if !seed_dest_and_poi(&slug, &poi) {
        return;
    }

    // two items, different categories (order: 名產 before 和菓子 if we use ASCII... use explicit categories)
    let (ok_a, _, e_a) = run(&[
        "add-omiyage", &slug, "item_b",
        "--name", "Banana Cake", "--category", "和菓子",
        "--buy-at", &poi,
        "--item-source-url", "https://example.com/banana",
        "--item-confidence", "verified",
        "--location-source-url", "https://example.com/floor-b",
        "--location-confidence", "verified",
        "--notes", "classic",
    ]);
    assert!(ok_a, "{e_a}");
    let (ok_b, _, e_b) = run(&[
        "add-omiyage", &slug, "item_a",
        "--name", "Senbei Pack", "--category", "名產",
        "--buy-at", &poi,
        "--item-source-url", "https://example.com/senbei",
        "--item-confidence", "reviewed",
        "--location-source-url", "https://example.com/floor-a",
        "--location-confidence", "reviewed",
    ]);
    assert!(ok_b, "{e_b}");

    let (ok, stdout, e) = run(&["query-omiyage", "--slug", &slug]);
    assert!(ok, "query; stderr={e}");
    // item_id printed
    assert!(stdout.contains("item_a") && stdout.contains("item_b"), "item_ids: {stdout}");
    // both category headers / labels present
    assert!(stdout.contains("名產") && stdout.contains("和菓子"), "categories: {stdout}");
    // POI title/area/station from join
    assert!(stdout.contains("Test Depachika"), "poi title: {stdout}");
    assert!(stdout.contains("namba") || stdout.contains("Namba"), "area/station: {stdout}");
    // dual provenance: item URLs once + location URLs
    assert!(stdout.contains("https://example.com/banana"), "item provenance: {stdout}");
    assert!(stdout.contains("https://example.com/floor-b"), "loc provenance: {stdout}");
    // nullable address → '—' (seed left address NULL)
    assert!(stdout.contains('—') || stdout.contains("—"), "nullable dash: {stdout}");
    // deterministic order from repo ORDER BY i.category, i.name, i.item_id, ...
    // Lock relative order of the two category labels in stdout (whichever UTF-8 sort puts first).
    let i_meisan = stdout.find("名產").expect("名產");
    let i_wagashi = stdout.find("和菓子").expect("和菓子");
    let cat_order_ok = if "名產" < "和菓子" {
        i_meisan < i_wagashi
    } else {
        i_wagashi < i_meisan
    };
    assert!(cat_order_ok, "category order must follow ORDER BY i.category; stdout={stdout}");
}

#[test]
fn query_omiyage_unknown_vs_empty() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = nanos();
    let slug = format!("omi_empty_{n}");
    let _g = Guard::new({
        let s = slug.clone();
        move || omi_teardown(&s)
    });
    omi_teardown(&slug);
    // known dest, zero omiyage rows
    if db_exec(&format!(
        "INSERT INTO destination_config (slug, display_name, timezone, currency, origin) \
           VALUES ('{slug}', 'Empty Omi', 'Asia/Tokyo', 'JPY', 'taiwan');"
    ))
    .is_none()
    {
        return;
    }

    let (ok_empty, out_e, err_e) = run(&["query-omiyage", "--slug", &slug]);
    assert!(!ok_empty, "empty dest must fail");
    let msg_empty = format!("{out_e}{err_e}");
    assert!(
        msg_empty.contains("no sourced") || msg_empty.contains("no omiyage") || msg_empty.contains("empty"),
        "empty message: {msg_empty}"
    );

    let (ok_unk, out_u, err_u) = run(&["query-omiyage", "--slug", "no_such_dest_omiyage_zzz"]);
    assert!(!ok_unk, "unknown dest must fail");
    let msg_unk = format!("{out_u}{err_u}");
    assert!(
        msg_unk.contains("unknown") || msg_unk.contains("not found") || msg_unk.contains("destination_config"),
        "unknown message: {msg_unk}"
    );
    // different messages
    assert_ne!(
        msg_empty.trim(),
        msg_unk.trim(),
        "unknown vs empty must differ"
    );
}

#[test]
fn query_omiyage_corrupt_orphan_fails() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = nanos();
    let slug = format!("omi_orph_{n}");
    let _g = Guard::new({
        let s = slug.clone();
        move || omi_teardown(&s)
    });
    omi_teardown(&slug);
    if db_exec(&format!(
        "INSERT INTO destination_config (slug, display_name, timezone, currency, origin) \
           VALUES ('{slug}', 'Orph', 'Asia/Tokyo', 'JPY', 'taiwan'); \
         INSERT INTO destination_omiyage_items \
           (slug,item_id,name,category,notes,source_url,fetched_at,confidence) \
           VALUES ('{slug}','orphan_item','O','名產',NULL,'https://example.com/i','2026-07-12T00:00:00Z','verified'); \
         INSERT INTO destination_omiyage_locations \
           (slug,item_id,poi_id,purchase_note,source_url,fetched_at,confidence) \
           VALUES ('{slug}','orphan_item','missing_poi',NULL,'https://example.com/l','2026-07-12T00:00:00Z','verified');"
    ))
    .is_none()
    {
        return;
    }

    let (ok, _, e) = run(&["query-omiyage", "--slug", &slug]);
    assert!(!ok, "orphan POI must fail query, not silently omit; stderr={e}");
}

#[test]
fn query_omiyage_rejects_plan_id_help_and_typo() {
    // --help: no Turso needed if parse is pre-connect; still fine if connect later
    let (ok_h, out_h, err_h) = run(&["query-omiyage", "--help"]);
    assert!(ok_h, "help should succeed");
    let help = format!("{out_h}{err_h}");
    assert!(
        help.contains("Usage") || help.contains("usage") || help.contains("query-omiyage"),
        "help text: {help}"
    );

    let (ok_p, _, e_p) = run(&["query-omiyage", "--slug", "x", "--plan-id", "p"]);
    assert!(!ok_p, "--plan-id rejected; stderr={e_p}");
    assert!(
        e_p.contains("plan-id") || e_p.contains("--plan-id"),
        "friendly plan-id: {e_p}"
    );

    let (ok_t, _, e_t) = run(&["query-omiyage", "--slug", "x", "--slugg", "y"]);
    assert!(!ok_t, "typo flag rejected; stderr={e_t}");
}
```

- [ ] **Step 2: Run to verify fail.**

```bash
cd rust && cargo test -p travel-cli --test omiyage query_omiyage -- --test-threads=1 --nocapture
```

- [ ] **Step 3: Implement `query_omiyage.rs`**

- `--help` / `-h` → usage.
- Parse: require `--slug`; **explicit `--plan-id` arm** with friendly reject; catch-all unknown flags.
- `connect_read` → if `!config_slug_exists` → fail loud (unknown dest message) → else `query_omiyage(conn, slug)`:
  - empty → fail loud (`Error: no sourced omiyage for '<slug>'` — distinct from unknown).
  - any row with NULL/missing poi_title (orphan) → fail loud (corrupt row; do not omit).
  - else group by category; per item print item_id/name/notes + item provenance once; per location print POI title/poi_id/area/station/address-or-`—`/hours-or-`—`/purchase_note + location provenance. Order from repo ORDER BY.
- Register `mod query_omiyage;` + main.rs arm. Update `docs/reference/CLI.md` with both `add-omiyage` and `query-omiyage`.

- [ ] **Step 4: Build + serialized tests + live smoke (optional against test slug).**

```bash
cd rust && cargo build -p travel-cli && cargo test -p travel-cli --test omiyage -- --test-threads=1 --nocapture
```

- [ ] **Step 5: Commit**

```bash
git add rust/crates/travel-cli/src/query_omiyage.rs rust/crates/travel-cli/src/main.rs \
  rust/crates/travel-cli/tests/omiyage.rs docs/reference/CLI.md
git commit -F - <<'EOF'
feat(omiyage): query-omiyage view + CLI.md

Grouped plain-text view with item_id, dual provenance, nullable —, deterministic
order; fail-loud unknown vs empty vs corrupt; reject --plan-id; --help parity.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Task 5 (commit 5) — `validate data` omiyage invariants

**Files:** `rust/crates/travel-cli/src/validate.rs`, tests.

**Interfaces:** add a **dedicated** validate path for omiyage (e.g. `async fn validate_omiyage(issues: &mut Vec<Issue>)`), called from the main validate flow next to `validate_holiday_calendars` / `validate_destinations` (`validate.rs:65–68`).  

**Do NOT** add `destination_omiyage_*` to the empty-table-is-error checklist at **`validate.rs:1005`** — empty omiyage is **valid** (spec L81).  

Model provenance-style Error issues on **`validate.rs:969`** (`url_empty || fetched_empty -> Error`), not the ungeocoded WARN pattern alone.

**Full invariant list (spec L73–81) — each a separate Error when violated:**

1. **item.slug NOT IN destination_config** (spec L74) — `SELECT` items whose slug is missing from `destination_config`.
2. location whose parent `(slug,item_id)` has no matching item (orphan location).
3. location whose `(slug,poi_id)` has no matching `destination_pois` (orphan seller / wrong-slug POI).
4. item with zero locations.
5. item identity empty/blank: `name` / `category` (and item_id if ever blank).
6. item or location with blank/NULL required provenance: `source_url` / `fetched_at` / `confidence`.
7. confidence NOT IN (`verified`, `reviewed`) on either table.
8. **EMPTY omiyage tables → no error** (do not push any issue solely for zero rows).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn validate_data_catches_omiyage_orphans_and_missing_provenance() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let n = nanos();
    let slug = format!("omi_val_{n}");
    let poi = format!("omi_poi_val_{n}");
    let _g = Guard::new({
        let s = slug.clone();
        move || omi_teardown(&s)
    });
    omi_teardown(&slug);

    // --- empty stays valid: no omiyage rows for this slug (and we don't require global non-empty) ---
    // Run validate and only assert our slug doesn't appear in omiyage errors; global exit may still
    // fail on unrelated live data — so assert on stdout/stderr *lines about omiyage* for our slug.

    // Seed a multi-corrupt state via db_exec:
    if db_exec(&format!(
        "INSERT INTO destination_config (slug, display_name, timezone, currency, origin) \
           VALUES ('{slug}', 'Val', 'Asia/Tokyo', 'JPY', 'taiwan'); \
         INSERT INTO destination_pois (slug, poi_id, title, source_url, fetched_at, confidence) \
           VALUES ('{slug}', '{poi}', 'P', 'https://example.com/p', '2026-07-12T00:00:00Z', 'verified'); \
         /* (a) location with missing parent item */ \
         INSERT INTO destination_omiyage_locations \
           (slug,item_id,poi_id,purchase_note,source_url,fetched_at,confidence) \
           VALUES ('{slug}','ghost_item','{poi}',NULL,'https://example.com/l','2026-07-12T00:00:00Z','verified'); \
         /* (b) location with missing POI */ \
         INSERT INTO destination_omiyage_items \
           (slug,item_id,name,category,notes,source_url,fetched_at,confidence) \
           VALUES ('{slug}','has_bad_poi','N','名產',NULL,'https://example.com/i','2026-07-12T00:00:00Z','verified'); \
         INSERT INTO destination_omiyage_locations \
           (slug,item_id,poi_id,purchase_note,source_url,fetched_at,confidence) \
           VALUES ('{slug}','has_bad_poi','missing_poi',NULL,'https://example.com/l2','2026-07-12T00:00:00Z','verified'); \
         /* (c) item with no location */ \
         INSERT INTO destination_omiyage_items \
           (slug,item_id,name,category,notes,source_url,fetched_at,confidence) \
           VALUES ('{slug}','lonely','L','名產',NULL,'https://example.com/i2','2026-07-12T00:00:00Z','verified'); \
         /* (d) blank source_url on an item */ \
         INSERT INTO destination_omiyage_items \
           (slug,item_id,name,category,notes,source_url,fetched_at,confidence) \
           VALUES ('{slug}','blank_url','B','名產',NULL,'','2026-07-12T00:00:00Z','verified'); \
         INSERT INTO destination_omiyage_locations \
           (slug,item_id,poi_id,purchase_note,source_url,fetched_at,confidence) \
           VALUES ('{slug}','blank_url','{poi}',NULL,'https://example.com/l3','2026-07-12T00:00:00Z','verified'); \
         /* (e) confidence='guess' */ \
         INSERT INTO destination_omiyage_items \
           (slug,item_id,name,category,notes,source_url,fetched_at,confidence) \
           VALUES ('{slug}','bad_conf','C','名產',NULL,'https://example.com/i3','2026-07-12T00:00:00Z','guess'); \
         INSERT INTO destination_omiyage_locations \
           (slug,item_id,poi_id,purchase_note,source_url,fetched_at,confidence) \
           VALUES ('{slug}','bad_conf','{poi}',NULL,'https://example.com/l4','2026-07-12T00:00:00Z','verified');"
    ))
    .is_none()
    {
        return;
    }

    // (f) item.slug NOT IN destination_config — arm Guard FIRST, then seed phantom slug
    let phantom = format!("omi_phantom_{n}");
    let _g2 = Guard::new({
        let p = phantom.clone();
        move || omi_teardown(&p)
    });
    if db_exec(&format!(
        "INSERT INTO destination_omiyage_items \
           (slug,item_id,name,category,notes,source_url,fetched_at,confidence) \
           VALUES ('{phantom}','ph','P','名產',NULL,'https://example.com/ph','2026-07-12T00:00:00Z','verified'); \
         INSERT INTO destination_omiyage_locations \
           (slug,item_id,poi_id,purchase_note,source_url,fetched_at,confidence) \
           VALUES ('{phantom}','ph','no_poi',NULL,'https://example.com/phl','2026-07-12T00:00:00Z','verified');"
    ))
    .is_none()
    {
        return;
    }

    let (ok, stdout, stderr) = run(&["validate", "data"]);
    let all = format!("{stdout}{stderr}");
    // Do not assert global exit code alone (live DB may have other issues). Assert each class is reported:
    assert!(
        all.contains("ghost_item") || all.contains("orphan") || all.contains("parent"),
        "orphan location/item: {all}"
    );
    assert!(
        all.contains("missing_poi") || all.contains("has_bad_poi") || all.contains("poi"),
        "orphan seller POI: {all}"
    );
    assert!(
        all.contains("lonely") || all.contains("no location") || all.contains("without location"),
        "item without location: {all}"
    );
    assert!(
        all.contains("blank_url") || all.contains("source_url") || all.contains("provenance"),
        "blank provenance: {all}"
    );
    assert!(
        all.contains("bad_conf") || all.contains("guess") || all.contains("confidence"),
        "bad confidence: {all}"
    );
    assert!(
        all.contains(&phantom) || all.contains("destination_config") || all.contains("slug"),
        "item.slug not in destination_config: {all}"
    );
    let _ = ok; // global may be non-zero; class coverage is the lock
}

#[test]
fn validate_data_empty_omiyage_stays_valid_for_slug() {
    // Empty omiyage tables must NOT produce an empty-table Error (spec L81).
    // Do NOT add omiyage to validate.rs:1005 checklist.
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless");
        return;
    };
    let _ = run(&["db", "migrate"]);
    let (_, stdout, stderr) = run(&["validate", "data"]);
    let all = format!("{stdout}{stderr}");
    for line in all.lines() {
        let lower = line.to_lowercase();
        if lower.contains("omiyage")
            && (lower.contains("is empty")
                || lower.contains("no rows")
                || lower.contains("no live seeder"))
        {
            panic!("empty omiyage must not be an integrity error: {line}");
        }
    }
}
```

- [ ] **Step 2: Run to verify fail** (validate doesn't check omiyage yet — corrupt seeds produce no omiyage Error lines).

```bash
cd rust && cargo test -p travel-cli --test omiyage validate_data_catches -- --test-threads=1 --nocapture
```

- [ ] **Step 3: Add the invariants** in `validate.rs`

```rust
// In the main validate flow (near validate_holiday_calendars):
validate_omiyage(&mut issues).await;

async fn validate_omiyage(issues: &mut Vec<Issue>) {
    // connect_read; on connect fail push Error and return
    // For each invariant class, run a SELECT that finds violations and push Issue {
    //   category: "omiyage".into(),
    //   severity: Severity::Error,
    //   message: ...,
    //   file: Some("turso:destination_omiyage_*".into()),
    //   line: None,
    // }
    // NEVER: if COUNT(*)==0 { push empty-table error }
}
```

- [ ] **Step 4: Build + serialized tests.** Confirm `validate data` on the real DB does not invent empty-omiyage errors (no pre-existing omiyage rows is fine).

```bash
cd rust && cargo build -p travel-cli && cargo test -p travel-cli --test omiyage -- --test-threads=1 --nocapture
```

- [ ] **Step 5: Commit**

```bash
git add rust/crates/travel-cli/src/validate.rs rust/crates/travel-cli/tests/omiyage.rs
git commit -F - <<'EOF'
feat(omiyage): validate data invariants (dedicated path, empty OK)

Catch slug∉config, orphan locations×2, item-without-location, blank identity,
missing provenance, bad confidence. Empty omiyage tables remain valid — not on
the empty-table checklist.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Live smoke (after all commits) — real omiyage on a real destination

```bash
cd /home/yanggf/b/travel-2026
export TRAVEL_TURSO_URL=... READ/WRITE tokens ...
# a real, gwebcdb-verifiable item + a real existing seller POI (e.g. tokyo daimaru_tokyo sells Tokyo Banana):
./bin/travel add-omiyage tokyo_2026 tokyo_banana --name "Tokyo Banana" --category "和菓子" --buy-at daimaru_tokyo \
  --item-source-url "https://www.tokyobanana.jp/" --item-confidence verified \
  --location-source-url "<daimaru floor guide / listing URL>" --location-confidence verified
./bin/travel query-omiyage --slug tokyo_2026
./bin/travel validate data   # stays clean of omiyage Errors for this row
```
Expected: query groups by category, shows Tokyo Banana at Daimaru Tokyo (area/station/hours from the POI join), both provenance sources. (Only commit real, verified rows — do NOT leave test/fake omiyage in prod; if this smoke writes a real verified row it may stay, otherwise delete it.)

## Acceptance

Per the spec's acceptance section:

| Area | Criteria |
|------|----------|
| Schema | 2 tables via `exec_lenient` CREATE IF NOT EXISTS; schema.sql mirror; PKs `(slug,item_id)` / `(slug,item_id,poi_id)`; all TEXT; no JSON |
| add-omiyage | slug-keyed; no audit even with `$TRAVEL_PLAN_ID`; **atomic** `write_item_and_location`; existing-item = location-only + SUPPLIED-flag match; same-seller re-add refreshes `fetched_at`; full fail-loud matrix; friendly `--plan-id` reject; HTTP(S) prefix |
| query-omiyage | grouped; item_id + dual provenance; nullable `—`; deterministic order; unknown vs empty vs corrupt fail loud |
| validate | dedicated path; **8 classes including item.slug∉destination_config**; empty tables valid; **not** on `:1005` checklist |
| Tests | full seed→invoke→assert; Guard-first; `scalar()` API; real pois columns; serialized background; zero leaks |

All behavior locks green serialized; global-row teardown leaves zero leaked rows.
