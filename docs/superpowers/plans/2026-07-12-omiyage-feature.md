# 伴手禮推薦 feature Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add omiyage (souvenir) recommendation + purchase-location capability: two normalized reference tables, a slug-keyed `add-omiyage` write command + `query-omiyage` view (like add-transit — no --plan-id, no audit), reusing existing `destination_pois` as sellers, with real provenance and validate-data invariants.

**Architecture:** 5 tasks: (1) schema + db_migrate for 2 tables; (2) `travel-db::repo::omiyage` DAL (existence checks + atomic item/location write + query read); (3) `add-omiyage` command; (4) `query-omiyage` command; (5) `validate data` invariants. Each = one testable deliverable.

**Tech Stack:** Rust (travel-cli command layer + travel-db repo layer), libsql/Turso, real-Turso integration tests on `tests/common/mod.rs`.

## Global Constraints

- **No hardcode / Turso-only / no JSON in RDB / agent-first plain text / fail loud** — per the spec `docs/superpowers/specs/2026-07-12-omiyage-recommendation-feature.md` (authoritative — read it).
- **Slug-keyed GLOBAL reference data, NO audit triad** — `add-omiyage` writes NO plan_events/operation_runs/plans.version, even with `$TRAVEL_PLAN_ID` set. Mirrors `add-transit` (`add_transit.rs:1-15,30-75`).
- **db_migrate.rs creates the tables (idempotent `CREATE TABLE IF NOT EXISTS`); schema.sql mirrors the DDL.** Not both create.
- **Exact write contract (spec):** begin tx → validate slug + same-slug POI → read (slug,item_id) → absent: insert full item then location; present: DO NOT write item (bundle optional, if given must MATCH else fail), upsert location only → commit only on expected affected-row counts, else no partial write.
- **confidence ∈ {verified, reviewed}** on both tables; source_url required + HTTP(S); fetched_at auto-stamped (`chrono::Utc::now().to_rfc3339_opts(...)` like add_transit.rs:37).
- **CRITICAL test teardown** — these are non-plan-keyed global rows; `common::teardown_plan` will NOT clean them. Every test uses a UNIQUE slug + a panic-safe `common::Guard` whose closure deletes in dependency order: `destination_omiyage_locations` → `destination_omiyage_items` → seeded `destination_pois`/`destination_config` (via `common::db_exec_teardown`). Defensive pre-clean before the Guard. Arm the Guard right after ids are bound; never a trailing teardown.
- **Serialized real-Turso tests in the BACKGROUND** (`--test-threads=1`); a foreground timeout SIGTERMs mid-run and the Guard Drop never fires (leaks a prod row).
- **Pipeline** — Grok 4.5 implements task-by-task; Claude reviews every line + corroborates vs source + verifies serialized. Codex reviews plan/test-plan when quota returns; otherwise Grok 4.5 assists reviews but Claude remains the quality gate. Commit explicit pathspecs only.

---

## File Structure

- `rust/crates/travel-cli/src/db_migrate.rs` — add 2 `CREATE TABLE IF NOT EXISTS` (near destination_transit at :1410). (Task 1)
- `scripts/schema.sql` — mirror the 2 table DDLs. (Task 1)
- `rust/crates/travel-db/src/repo/omiyage.rs` — NEW repo: `item_exists`, `poi_exists_for_slug` (or reuse), `insert_item`, `upsert_location`, `read_item` (for match check), `query_omiyage` (join read). (Task 2)
- `rust/crates/travel-db/src/repo/mod.rs` — `pub mod omiyage;`. (Task 2)
- `rust/crates/travel-cli/src/add_omiyage.rs` — NEW command. (Task 3)
- `rust/crates/travel-cli/src/query_omiyage.rs` — NEW command. (Task 4)
- `rust/crates/travel-cli/src/main.rs` — 2 dispatch arms (like add-transit). (Tasks 3+4)
- `rust/crates/travel-cli/src/validate.rs` — omiyage invariants in `validate data`. (Task 5)
- `rust/crates/travel-cli/tests/omiyage.rs` — NEW behavior-lock test file (all tasks add cases here).
- `docs/reference/CLI.md` — document both commands. (Task 4)

---

## Task 1 (commit 1) — schema + migration for the 2 tables

**Files:**
- Modify: `rust/crates/travel-cli/src/db_migrate.rs` (add 2 CREATE near :1410), `scripts/schema.sql`
- Test: `rust/crates/travel-cli/tests/omiyage.rs` (new)

- [ ] **Step 1: Write the failing test**

Create `rust/crates/travel-cli/tests/omiyage.rs`. Assert the 2 tables exist with the right columns after `db migrate`.

```rust
mod common;
use common::{bin, db_exec, is_credless, nanos};

fn run(args: &[&str]) -> (bool, String, String) {
    let out = std::process::Command::new(bin()).args(args).output().expect("run");
    (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn omiyage_tables_exist_after_migrate() {
    let Some(_) = db_exec("SELECT 1") else { eprintln!("credless"); return; };
    let _ = run(&["db", "migrate"]);
    // both tables queryable with the specified columns (empty is fine)
    assert!(db_exec("SELECT slug,item_id,name,category,notes,source_url,fetched_at,confidence FROM destination_omiyage_items LIMIT 0").is_some(),
        "destination_omiyage_items must have the 8 columns");
    assert!(db_exec("SELECT slug,item_id,poi_id,purchase_note,source_url,fetched_at,confidence FROM destination_omiyage_locations LIMIT 0").is_some(),
        "destination_omiyage_locations must have the 7 columns");
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cd rust && cargo test -p travel-cli --test omiyage omiyage_tables_exist -- --test-threads=1 --nocapture
```
Expected: FAIL — `no such table: destination_omiyage_items`.

- [ ] **Step 3: Add the CREATE statements in db_migrate.rs**

Near `destination_transit` (`db_migrate.rs:1410`), add two `migrate_exec(...)` calls (match the surrounding call style — they wrap a raw SQL string). All columns TEXT:

```rust
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
```
```rust
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
```
(Use the EXACT wrapper the neighboring destination_transit CREATE uses — read :1408-1424 and copy the call shape.)

- [ ] **Step 4: Mirror in schema.sql**

Add both `CREATE TABLE destination_omiyage_items (...)` / `destination_omiyage_locations (...)` to `scripts/schema.sql` in the destination-reference section (near destination_transit), same column order, `PRIMARY KEY` as above.

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
fetched_at,confidence). db_migrate creates (idempotent); schema.sql mirrors.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Task 2 (commit 2) — `travel-db::repo::omiyage` DAL

**Files:**
- Create: `rust/crates/travel-db/src/repo/omiyage.rs`
- Modify: `rust/crates/travel-db/src/repo/mod.rs` (`pub mod omiyage;`)
- Test: `rust/crates/travel-cli/tests/omiyage.rs` (add a repo-level round-trip through the binary once Task 3/4 exist — for now, unit-test the SQL via `db_exec` seeding + a thin CLI or defer to Task 3)

**Interfaces (Produces — the command layer consumes these):**
- `async fn config_slug_exists(conn, slug) -> Result<bool, String>` — `SELECT 1 FROM destination_config WHERE slug=?1`.
- `async fn poi_exists_for_slug(conn, slug, poi_id) -> Result<bool, String>` — `SELECT 1 FROM destination_pois WHERE slug=?1 AND poi_id=?2` (SQL verbatim from destination_ref.rs:271).
- `async fn read_item(conn, slug, item_id) -> Result<Option<OmiyageItem>, String>` — for existence + match check. `struct OmiyageItem { name, category, notes, source_url, confidence }` (fetched_at excluded from match).
- `async fn insert_item(conn, slug, item_id, name, category, notes: Option<&str>, source_url, confidence, fetched_at) -> Result<u64, String>` — plain `INSERT` (NOT REPLACE — caller guarantees absence). Returns affected rows.
- `async fn upsert_location(conn, slug, item_id, poi_id, purchase_note: Option<&str>, source_url, confidence, fetched_at) -> Result<u64, String>` — `INSERT OR REPLACE INTO destination_omiyage_locations` on (slug,item_id,poi_id). Returns affected rows.
- `async fn query_omiyage(conn, slug) -> Result<Vec<OmiyageRow>, String>` — join items ⨝ locations ⨝ destination_pois, ordered category, name, item_id, poi title, poi_id. `struct OmiyageRow { item_id, name, category, item_notes, item_source_url, item_confidence, item_fetched_at, poi_id, poi_title, area, station, address: Option, hours: Option, purchase_note: Option, loc_source_url, loc_confidence, loc_fetched_at }`. LEFT JOIN destination_pois so a missing POI is detectable; but per spec a location's POI must exist (validate catches orphans) — use INNER JOIN and let query fail-loud-empty if a row is orphaned? NO: use LEFT JOIN and surface orphan as an error in the command (corrupt row → fail). Decide in the command layer; the repo returns rows incl. NULL poi fields for orphans so the command can fail.

- [ ] **Step 1: Write the failing test** (repo exercised via the query command in Task 4; for Task 2 alone, a focused test that seeds items+locations+pois via `db_exec` then asserts `query_omiyage` ordering/join by invoking a tiny path — simplest is to fold Task 2's lock into Task 4's query test. Mark Task 2 as "compiles + unit round-trip via Task 4". If a standalone repo test is wanted, seed via db_exec and assert the JOIN returns expected rows in order.)

> Because the repo has no CLI surface until Task 3/4, the cleanest TDD is: write the repo fns, then Task 3's add-omiyage test + Task 4's query test exercise them end-to-end. Task 2's own step is: implement the repo, `cargo build -p travel-db` clean, and a `#[tokio::test]` in `travel-db` (if the crate has tests) OR defer the behavior lock to Tasks 3/4. Keep Task 2 to "repo compiles + is called by Task 3".

- [ ] **Step 2: Implement `repo/omiyage.rs`**

Model each fn on `repo::destination_ref` (existence SELECTs at :271; INSERT OR REPLACE at :317-348). `query_omiyage` SQL:
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
Map NULL p.* (orphan location) to Option fields so the command can fail-loud on a corrupt row.

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
feat(omiyage): travel-db repo::omiyage DAL (existence, atomic write, join query)

config/poi existence checks, read_item (for match), insert_item (plain INSERT),
upsert_location (INSERT OR REPLACE), query_omiyage (items ⨝ locations ⨝
destination_pois, deterministic order, NULL poi surfaced for orphan detection).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Task 3 (commit 3) — `add-omiyage` command

**Files:**
- Create: `rust/crates/travel-cli/src/add_omiyage.rs`
- Modify: `rust/crates/travel-cli/src/main.rs` (dispatch arm), `rust/crates/travel-cli/src/module registration` (add `mod add_omiyage;`)
- Test: `rust/crates/travel-cli/tests/omiyage.rs`

**Interfaces:**
- Consumes: `repo::omiyage::{config_slug_exists, poi_exists_for_slug, read_item, insert_item, upsert_location}`.
- Produces: `travel add-omiyage <slug> <item_id> --buy-at <poi_id> --location-source-url <url> --location-confidence verified|reviewed [--name] [--category] [--item-source-url] [--item-confidence] [--notes] [--purchase-note]`.

- [ ] **Step 1: Write the failing tests** (the core behavior locks — every one uses a UNIQUE slug + Guard teardown in dependency order)

```rust
use common::{db_exec_teardown, Guard, seed_plan};
// helper: seed a destination_config + one destination_pois seller, return (slug, poi)
fn seed_dest_and_poi(n: u128) -> (String, String) {
    let slug = format!("omi_dest_{n}");
    let poi = format!("omi_poi_{n}");
    db_exec(&format!("INSERT INTO destination_config (slug,display_name,timezone,currency,origin) VALUES ('{slug}','Omi','Asia/Tokyo','JPY','TPE')"));
    db_exec(&format!("INSERT INTO destination_pois (slug,poi_id,title,area,nearest_station,duration_min,booking_required,cost_estimate,hours,source) VALUES ('{slug}','{poi}','Test Depachika','namba','Namba',60,0,3000,'10:00-20:00','seed')"));
    (slug, poi)
}
fn omi_teardown(slug: &str) {
    db_exec_teardown(&format!("DELETE FROM destination_omiyage_locations WHERE slug='{slug}'"));
    db_exec_teardown(&format!("DELETE FROM destination_omiyage_items WHERE slug='{slug}'"));
    db_exec_teardown(&format!("DELETE FROM destination_pois WHERE slug='{slug}'"));
    db_exec_teardown(&format!("DELETE FROM destination_config WHERE slug='{slug}'"));
}

#[test]
fn add_omiyage_creates_item_and_location() {
    let Some(_) = db_exec("SELECT 1") else { eprintln!("credless"); return; };
    let _ = run(&["db","migrate"]);
    let n = nanos();
    let (slug, poi) = { let x = seed_dest_and_poi(n); x };
    let _g = Guard::new({ let s = slug.clone(); move || omi_teardown(&s) });
    // (pre-clean already done by fresh unique slug)
    let (ok,_o,e) = run(&["add-omiyage",&slug,"tokyo_banana","--name","Tokyo Banana","--category","和菓子","--buy-at",&poi,"--item-source-url","https://www.tokyobanana.jp/","--item-confidence","verified","--location-source-url","https://example.com/floor","--location-confidence","verified"]);
    assert!(ok, "add-omiyage should succeed; err={e}");
    let it = db_exec(&format!("SELECT COUNT(*) FROM destination_omiyage_items WHERE slug='{slug}' AND item_id='tokyo_banana'")).unwrap();
    assert_eq!(it[0][0], "1");
    let lo = db_exec(&format!("SELECT COUNT(*) FROM destination_omiyage_locations WHERE slug='{slug}' AND item_id='tokyo_banana' AND poi_id='{poi}'")).unwrap();
    assert_eq!(lo[0][0], "1");
}

#[test]
fn add_omiyage_second_seller_keeps_item_unchanged() {
    // seed dest + 2 pois; add item+seller1; add seller2 (item bundle omitted) → 2 locations, item byte-identical.
    // assert item row's name/category/source_url unchanged after the 2nd add.
}

#[test]
fn add_omiyage_fail_loud_matrix() {
    // unknown slug; unknown poi; blank name on new item; missing --location-source-url;
    // invalid --location-confidence 'guess'; non-http source url; --plan-id present; unknown flag.
    // each → non-zero exit + no rows written.
}

#[test]
fn add_omiyage_no_audit_even_with_travel_plan_id() {
    // set TRAVEL_PLAN_ID; add-omiyage; assert no operation_runs / plan_events / plans.version bump for that plan.
}
```
Write ALL the fail-loud cases explicitly (one assert block each). Model the command output/exit like add-transit.

- [ ] **Step 2: Run to verify fail** (command doesn't exist → unknown command).

- [ ] **Step 3: Implement `add_omiyage.rs`**

Structure like `add_transit.rs`: `--help` guard (`wants_help`) → `reject_unknown_flags` (owned: `--buy-at --location-source-url --location-confidence --name --category --item-source-url --item-confidence --notes --purchase-note`; reject `--plan-id` explicitly) → parse (2 positionals slug+item_id; validate confidence ∈ {verified,reviewed}; validate source URLs are http(s); reject blank required) → `connect_write` → validate `config_slug_exists` + `poi_exists_for_slug` (fail loud) → `read_item`:
  - None → require full item bundle (name/category/item-source-url/item-confidence non-blank) → `insert_item` (assert affected==1) → `upsert_location` (assert affected==1).
  - Some(existing) → if any item-bundle flag given, require it to MATCH existing (else fail); do NOT write item → `upsert_location` only (assert affected==1).
  fetched_at via `chrono::Utc::now().to_rfc3339_opts(...)`. Plain-text success output (item, category, seller POI, confidence). NO audit triad. Register `mod add_omiyage;` + main.rs arm `[cmd, rest @ ..] if cmd == "add-omiyage" => add_omiyage::run(rest).await`.

- [ ] **Step 4: Build + run tests serialized**

```bash
cd rust && cargo build -p travel-cli && cargo test -p travel-cli --test omiyage -- --test-threads=1 --nocapture
```
Expected: all green.

- [ ] **Step 5: Commit** (`add_omiyage.rs`, `main.rs`, module reg, tests).

---

## Task 4 (commit 4) — `query-omiyage` command + CLI.md

**Files:**
- Create: `rust/crates/travel-cli/src/query_omiyage.rs`
- Modify: `main.rs` (arm + `mod query_omiyage;`), `docs/reference/CLI.md`
- Test: `rust/crates/travel-cli/tests/omiyage.rs`

**Interfaces:** Consumes `repo::omiyage::query_omiyage`. Produces `travel query-omiyage --slug <slug>`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn query_omiyage_round_trip_grouped_with_dual_provenance() {
    // seed dest+poi; add-omiyage 2 items in different categories; query.
    // assert: item_id printed; both category headers present; POI title/area/station shown;
    // item source_url shown once per item; location source_url shown per seller;
    // nullable address renders '—' when absent; deterministic order (category asc).
}
#[test]
fn query_omiyage_unknown_dest_fails_loud() { /* non-zero + message */ }
#[test]
fn query_omiyage_known_but_empty_dest_fails_loud() {
    // seed dest with NO omiyage rows → fail loud (different message than unknown dest)
}
#[test]
fn query_omiyage_corrupt_orphan_row_fails() {
    // insert a location whose poi_id doesn't exist (bypass add-omiyage via db_exec) → query fails, not silently omits.
}
#[test]
fn query_omiyage_rejects_plan_id_and_help_parity() { /* --plan-id rejected; --help prints Usage */ }
```

- [ ] **Step 2: Run to verify fail.**

- [ ] **Step 3: Implement `query_omiyage.rs`**

`--help` guard → `reject_unknown_flags` (owned: `--slug`; reject `--plan-id`) → require `--slug` → `connect_read` → `query_omiyage(conn, slug)`:
  - empty → fail loud (`Error: no sourced omiyage for '<slug>'` — distinct from unknown-dest; check `config_slug_exists` first to give unknown-dest a different message).
  - any row with NULL poi fields (orphan) → fail loud (corrupt row).
  - else group by category; per item print item_id/name/notes + item provenance once; per location print POI title/poi_id/area/station/address-or-`—`/hours-or-`—`/purchase_note + location provenance. Deterministic order from the repo's ORDER BY.
Register arm + `mod query_omiyage;`. Update `docs/reference/CLI.md` with both commands.

- [ ] **Step 4: Build + serialized tests + live smoke.**
- [ ] **Step 5: Commit.**

---

## Task 5 (commit 5) — `validate data` omiyage invariants

**Files:** `rust/crates/travel-cli/src/validate.rs`, tests.

**Interfaces:** adds omiyage checks to the existing `validate data` flow (model on the ungeocoded-POI check at validate.rs:849 + missing-provenance at :969).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn validate_data_catches_omiyage_orphans_and_missing_provenance() {
    // seed a corrupt state via db_exec: (a) location with missing parent item;
    // (b) location with missing POI; (c) item with no location; (d) blank source_url on an item;
    // (e) confidence='guess'. Run `validate data` → each surfaces as an Error line.
    // Also: an EMPTY omiyage table set → validate data stays clean (absence is not an error).
}
```

- [ ] **Step 2: Run to verify fail** (validate doesn't check omiyage yet).

- [ ] **Step 3: Add the invariants** in `validate.rs` (each a SELECT that finds violations, pushed as Error, mirroring :849/:969):
  - location whose `(slug,item_id)` has no matching item (orphan).
  - location whose `(slug,poi_id)` has no matching destination_pois (orphan seller).
  - item with zero locations.
  - item/location with blank/NULL required field (name/category/source_url/fetched_at/confidence).
  - confidence NOT IN ('verified','reviewed').
  - EMPTY tables → no error.

- [ ] **Step 4: Build + serialized tests.** Confirm `validate data` on the real DB stays clean (no pre-existing omiyage rows).

- [ ] **Step 5: Commit.**

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
./bin/travel validate data   # stays clean
```
Expected: query groups by category, shows Tokyo Banana at Daimaru Tokyo (area/station/hours from the POI join), both provenance sources. (Only commit real, verified rows — do NOT leave test/fake omiyage in prod; if this smoke writes a real verified row it may stay, otherwise delete it.)

## Acceptance

Per the spec's acceptance section: 2 tables (db_migrate creates), add-omiyage (slug-keyed, no audit, atomic, existing-item=location-only, fail-loud matrix, reject --plan-id), query-omiyage (grouped, item_id + dual provenance, nullable `—`, deterministic order, fail-loud on unknown/empty/corrupt), validate-data invariants (8 classes, empty stays valid). All behavior locks green serialized; global-row teardown leaves zero leaked rows.
