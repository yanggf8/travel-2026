# omiyage-worklist Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a READ-ONLY `omiyage-worklist --slug <dest>` command that discovers omiyage-tagged POIs, prints their notes verbatim as unverified hints + a verify worklist + an add-omiyage template, and writes NOTHING — then hook it into the Stage 3 skill so omiyage auto-generates in the flow (agent gwebcdb-verifies each candidate, then persists via the existing add-omiyage). No pending rows, no schema change.

**Architecture:** 2 tasks: (1) a repo DAL read fn (`omiyage_worklist_pois`) + the `omiyage-worklist` command + tests; (2) the Stage 3 skill-doc hook + CLI.md. The command mirrors `query_omiyage.rs`'s read-only structure exactly (connect_read → config_slug_exists → render; reject --plan-id; --help).

**Tech Stack:** Rust (travel-cli command + travel-db repo), libsql/Turso, real-Turso integration tests on `tests/common/mod.rs`.

## Global Constraints

- **NO-CHEAT (core):** the command writes NOTHING — no pending rows, no schema change, no confidence-enum change. POI notes are printed VERBATIM as hints, never parsed into item/seller facts. Canonical omiyage tables stay fact-only. Per spec `docs/superpowers/specs/2026-07-13-omiyage-worklist-auto-generate.md` (authoritative — read it).
- **Read-only reference command, no audit, no --plan-id** — mirrors `query_omiyage.rs` (connect_read, config_slug_exists, explicit --plan-id reject, --help).
- **CRITICAL test teardown** — the tests seed non-plan-keyed global rows (destination_config, destination_pois, destination_poi_tags, and the omiyage tables via add-omiyage for the already-sourced-count test). `common::teardown_plan` will NOT clean them. Every test uses a UNIQUE slug + a panic-safe `common::Guard` armed FIRST (before seed), deleting in dependency order: `destination_omiyage_locations` → `destination_omiyage_items` → `destination_poi_tags` → `destination_pois` → `destination_config` (via `common::db_exec_teardown`). Reuse the existing `omi_teardown` helper in tests/omiyage.rs but EXTEND it to also delete `destination_poi_tags` for the slug (worklist tests seed tags). Defensive pre-clean before the Guard.
- **Serialized real-Turso tests in the BACKGROUND** (`--test-threads=1`); a bare substring test run causes Turso BUSY false-failures.
- **Delegates: NO git operations** — implementers do NOT run git reset/add/commit/checkout/stash. Leave changes uncommitted; the parent commits.
- **Pipeline** — Grok 4.5 implements task-by-task; Claude reviews every line + corroborates vs source + verifies serialized (Codex reviews plan when quota allows; Claude is the quality gate regardless). Commit explicit pathspecs only.

---

## File Structure

- `rust/crates/travel-db/src/repo/omiyage.rs` — add `omiyage_worklist_pois(conn, slug) -> Result<Vec<WorklistPoi>, String>` (poi_tags⨝pois for tag='omiyage' + an already-sourced location count per POI). (Task 1)
- `rust/crates/travel-cli/src/omiyage_worklist.rs` — NEW command, mirrors `query_omiyage.rs`. (Task 1)
- `rust/crates/travel-cli/src/main.rs` — `mod omiyage_worklist;` + dispatch arm + print_usage. (Task 1)
- `rust/crates/travel-cli/tests/omiyage.rs` — new worklist tests; extend `omi_teardown` to delete destination_poi_tags. (Task 1)
- `src/skills/stage3-expand-itinerary/SKILL.md` — auto-run hook (prose). (Task 2)
- `docs/reference/CLI.md` — document omiyage-worklist. (Task 2)

---

## Task 1 (commit 1) — `omiyage-worklist` command + DAL

**Files:**
- Modify: `rust/crates/travel-db/src/repo/omiyage.rs` (add `WorklistPoi` struct + `omiyage_worklist_pois` fn near `query_omiyage` at :130)
- Create: `rust/crates/travel-cli/src/omiyage_worklist.rs`
- Modify: `rust/crates/travel-cli/src/main.rs`, `rust/crates/travel-cli/tests/omiyage.rs`

**Interfaces:**
- Consumes: `repo::omiyage::config_slug_exists` (omiyage.rs:81).
- Produces: `repo::omiyage::omiyage_worklist_pois(conn, slug) -> Result<Vec<WorklistPoi>, String>`. `struct WorklistPoi { poi_id, title, area, notes: Option<String>, already_sourced: i64 }`. Command `travel omiyage-worklist --slug <slug>`.

- [ ] **Step 1: Write the failing tests**

First, EXTEND `omi_teardown` in tests/omiyage.rs to also delete tags (worklist tests seed `destination_poi_tags`):
```rust
// in omi_teardown, add a line (order doesn't matter for tags — it's a leaf):
let _ = db_exec_teardown(&format!("DELETE FROM destination_poi_tags WHERE slug='{s}'"));
```
(Place it alongside the existing DELETEs; tags reference the POI but nothing references tags, so delete before or with pois.)

Add these tests (UNIQUE slug, Guard FIRST, seed a config + a POI + an omiyage tag):
```rust
// helper: seed config + poi + an omiyage tag with a notes hint. Returns false credless.
fn seed_omiyage_tagged_poi(slug: &str, poi: &str, notes: &str) -> bool {
    if db_exec("SELECT 1").is_none() { return false; }
    let s = slug.replace('\'', "''");
    let p = poi.replace('\'', "''");
    let nt = notes.replace('\'', "''");
    db_exec(&format!(
        "INSERT INTO destination_config (slug,display_name,timezone,currency,origin) VALUES ('{s}','Omi','Asia/Tokyo','JPY','taiwan');\
         INSERT INTO destination_pois (slug,poi_id,title,area,nearest_station,notes,source) VALUES ('{s}','{p}','Test Depachika','namba','Namba','{nt}','seed');\
         INSERT INTO destination_poi_tags (slug,poi_id,tag,sort_order) VALUES ('{s}','{p}','omiyage',0);"
    )).is_some()
}

#[test]
fn worklist_prints_tagged_poi_notes_and_template_writes_nothing() {
    let Some(_) = db_exec("SELECT 1") else { eprintln!("credless"); return; };
    let _ = run(&["db","migrate"]);
    let n = nanos();
    let slug = format!("omi_wl_{n}"); let poi = format!("omi_wlp_{n}");
    let _g = Guard::new({ let s = slug.clone(); move || omi_teardown(&s) });
    omi_teardown(&slug);
    if !seed_omiyage_tagged_poi(&slug, &poi, "Premium omiyage: Yoku Moku, Tokyo Banana") { return; }

    let (ok, out, e) = run(&["omiyage-worklist","--slug",&slug]);
    assert!(ok, "worklist should succeed; e={e}");
    assert!(out.contains(&poi), "prints poi_id: {out}");
    assert!(out.contains("Yoku Moku"), "prints notes verbatim: {out}");
    assert!(out.contains("WARNING") || out.contains("hint"), "warns notes are hints: {out}");
    assert!(out.contains("add-omiyage") && out.contains(&slug), "prints add-omiyage template with slug: {out}");
    assert!(out.contains("already sourced") || out.contains("0"), "shows already-sourced count: {out}");

    // WRITES NOTHING: no omiyage rows created for this slug.
    let items = db_exec(&format!("SELECT COUNT(*) FROM destination_omiyage_items WHERE slug='{slug}'")).unwrap();
    assert_eq!(items.scalar().as_deref(), Some("0"), "worklist must write NO items");
    let locs = db_exec(&format!("SELECT COUNT(*) FROM destination_omiyage_locations WHERE slug='{slug}'")).unwrap();
    assert_eq!(locs.scalar().as_deref(), Some("0"), "worklist must write NO locations");
}

#[test]
fn worklist_already_sourced_count_reflects_existing() {
    let Some(_) = db_exec("SELECT 1") else { eprintln!("credless"); return; };
    let _ = run(&["db","migrate"]);
    let n = nanos();
    let slug = format!("omi_wls_{n}"); let poi = format!("omi_wlsp_{n}");
    let _g = Guard::new({ let s = slug.clone(); move || omi_teardown(&s) });
    omi_teardown(&slug);
    if !seed_omiyage_tagged_poi(&slug, &poi, "Tokyo Banana") { return; }
    // add one real omiyage row at this POI via the real command
    let (ok_a,_,ea) = run(&["add-omiyage",&slug,"tokyo_banana","--name","Tokyo Banana","--category","和菓子","--buy-at",&poi,"--item-source-url","https://example.com/i","--item-confidence","verified","--location-source-url","https://example.com/l","--location-confidence","verified"]);
    assert!(ok_a, "{ea}");
    let (ok, out, e) = run(&["omiyage-worklist","--slug",&slug]);
    assert!(ok, "e={e}");
    // already-sourced count for this POI should be 1
    assert!(out.contains("already sourced here: 1") || out.contains(": 1"), "count reflects the 1 existing: {out}");
}

#[test]
fn worklist_no_tagged_poi_fails_loud() {
    let Some(_) = db_exec("SELECT 1") else { eprintln!("credless"); return; };
    let _ = run(&["db","migrate"]);
    let n = nanos(); let slug = format!("omi_wln_{n}");
    let _g = Guard::new({ let s = slug.clone(); move || omi_teardown(&s) });
    omi_teardown(&slug);
    // config exists but NO omiyage-tagged POI
    if db_exec(&format!("INSERT INTO destination_config (slug,display_name,timezone,currency,origin) VALUES ('{slug}','Omi','Asia/Tokyo','JPY','taiwan')")).is_none() { return; }
    let (ok, _o, e) = run(&["omiyage-worklist","--slug",&slug]);
    assert!(!ok, "no omiyage-tagged POI must fail loud");
    assert!(e.contains("no omiyage") || e.contains("no omiyage-tagged") || e.contains("tag"), "message: {e}");
}

#[test]
fn worklist_unknown_dest_and_plan_id_and_help() {
    let (ok_u,_o,e_u) = run(&["omiyage-worklist","--slug","no_such_dest_wl_zzz"]);
    assert!(!ok_u, "unknown dest fails");
    assert!(e_u.contains("unknown") || e_u.contains("destination_config"), "unknown msg: {e_u}");
    let (ok_p,_o,e_p) = run(&["omiyage-worklist","--slug","x","--plan-id","p"]);
    assert!(!ok_p, "--plan-id rejected");
    assert!(e_p.contains("plan-id"), "friendly: {e_p}");
    let (ok_h,out_h,err_h) = run(&["omiyage-worklist","--help"]);
    assert!(ok_h && (format!("{out_h}{err_h}").contains("Usage") || format!("{out_h}{err_h}").contains("omiyage-worklist")), "help");
    let (ok_t,_o,_e) = run(&["omiyage-worklist","--slug","x","--slugg","y"]);
    assert!(!ok_t, "typo flag rejected");
}
```

- [ ] **Step 2: Run to verify fail** (`omiyage-worklist` doesn't exist → unknown command).
```bash
cd rust && cargo test -p travel-cli --test omiyage worklist -- --test-threads=1 --nocapture
```

- [ ] **Step 3: Add the DAL fn** in `repo/omiyage.rs` (near query_omiyage at :130). Model the join + count on existing repo SQL:
```rust
pub struct WorklistPoi {
    pub poi_id: String,
    pub title: String,
    pub area: String,
    pub notes: Option<String>,
    pub already_sourced: i64,
}

/// POIs tagged 'omiyage' for a slug, with their notes (verbatim hint) and a count
/// of already-sourced omiyage locations at that POI. Read-only discovery for the worklist.
pub async fn omiyage_worklist_pois(conn: &Connection, slug: &str) -> Result<Vec<WorklistPoi>, String> {
    let mut rows = conn.query(
        "SELECT p.poi_id, p.title, p.area, p.notes, \
                (SELECT COUNT(*) FROM destination_omiyage_locations l WHERE l.slug=p.slug AND l.poi_id=p.poi_id) AS sourced \
         FROM destination_poi_tags t \
         JOIN destination_pois p ON p.slug=t.slug AND p.poi_id=t.poi_id \
         WHERE t.slug=?1 AND t.tag='omiyage' \
         ORDER BY p.poi_id",
        libsql::params![slug.to_string()],
    ).await.map_err(|e| format!("omiyage_worklist_pois failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().await.map_err(|e| format!("worklist row read failed: {e}"))? {
        out.push(WorklistPoi {
            poi_id: r.get(0).unwrap_or_default(),
            title: r.get(1).unwrap_or_default(),
            area: r.get(2).unwrap_or_default(),
            notes: opt_string(&r, 3),   // reuse the existing opt_string helper in this file
            already_sourced: r.get(4).unwrap_or(0),
        });
    }
    Ok(out)
}
```
(`opt_string` already exists at the bottom of omiyage.rs — reuse it.)

- [ ] **Step 4: Create `omiyage_worklist.rs`** — mirror `query_omiyage.rs` structure exactly:
```rust
// travel omiyage-worklist --slug <slug> — READ-ONLY discovery of omiyage-tagged
// POIs as an unverified research worklist. Writes NOTHING. Reference data — no
// --plan-id, no audit. The agent gwebcdb-verifies each candidate then persists
// via `add-omiyage`.
use travel_db::repo::omiyage::{self, WorklistPoi};

pub async fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") { println!("{}", usage()); return Ok(()); }
    let slug = parse_args(args)?;   // same parse as query_omiyage: require --slug, reject --plan-id + unknown flags
    let conn = crate::db::connect_read().await?;
    if !omiyage::config_slug_exists(&conn, &slug).await? {
        return Err(format!("Error: unknown destination '{slug}' — not in destination_config"));
    }
    let pois = omiyage::omiyage_worklist_pois(&conn, &slug).await?;
    if pois.is_empty() {
        return Err(format!("Error: no omiyage-tagged POIs for '{slug}' — tag shopping POIs with tag='omiyage' first, or add omiyage directly with: travel add-omiyage ..."));
    }
    render(&slug, &pois);
    Ok(())
}
```
`render` prints the spec's shape: a header `Omiyage research worklist: {slug}` + the WARNING line, then per POI: `POI {poi_id} — {title}`, `  area: {area}`, `  note hint: {notes or (none)}`, `  already sourced here: {already_sourced}`, the `VERIFY BEFORE ADDING:` two lines, and the `CONFIRM WITH:` add-omiyage template with `{slug}` and `--buy-at {poi_id}` filled and item/category/URLs/confidence as `<...>` placeholders. `usage`/`parse_args` copy query_omiyage.rs's (require --slug, explicit --plan-id reject, unknown-flag reject, arg_value). Register `mod omiyage_worklist;` + main.rs arm `[cmd, rest @ ..] if cmd == "omiyage-worklist" => omiyage_worklist::run(rest).await` + a print_usage VIEWS line near query-omiyage.

- [ ] **Step 5: Build + run tests serialized.**
```bash
cd rust && cargo build -p travel-cli && cargo test -p travel-cli --test omiyage -- --test-threads=1 --nocapture
```
All omiyage tests green (Task-1..5 of the prior feature + these 4 worklist tests). Verify 0 leaked rows for the test slug prefixes.

- [ ] **Step 6: Commit** (repo/omiyage.rs, omiyage_worklist.rs, main.rs, tests/omiyage.rs).

---

## Task 2 (commit 2) — Stage 3 skill hook + CLI.md

**Files:** `src/skills/stage3-expand-itinerary/SKILL.md`, `docs/reference/CLI.md`

- [ ] **Step 1: Add the Stage 3 auto-run hook**

In `src/skills/stage3-expand-itinerary/SKILL.md`, near the agent-first / no-cheat closing section, add a prose step: Stage 3 automatically runs `omiyage-worklist --slug <dest>`; for each candidate POI, the agent gwebcdb-verifies (1) an official item/product page and (2) an official branch/floor-guide page proving sale at that POI; only with both does it call `add-omiyage`; candidates it cannot verify are left out (honest gap — like an unverifiable restaurant). Finish with `query-omiyage --slug <dest>` + `validate data`. Emphasize no-cheat: never write an unsourced item/seller.

- [ ] **Step 2: Update CLI.md**

Document `omiyage-worklist --slug <slug>` in the reference-data section near add-omiyage/query-omiyage: "read-only research worklist — lists omiyage-tagged POIs + their notes (unverified hints) + an add-omiyage template; writes nothing; the agent gwebcdb-verifies then confirms via add-omiyage. Stage 3 runs this automatically."

- [ ] **Step 3: Commit** (SKILL.md, CLI.md).

---

## Live smoke (after both commits)

```bash
cd /home/yanggf/b/travel-2026
export TRAVEL_TURSO_URL=$(grep '^TURSO_URL=' .env | cut -d= -f2-)
export TRAVEL_TURSO_READ_TOKEN=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-)
./bin/travel omiyage-worklist --slug tokyo_2026
# then confirm it wrote nothing:
./bin/travel db exec "SELECT COUNT(*) FROM destination_omiyage_items WHERE slug='tokyo_2026'"   # unchanged
```
Expected: worklist for isetan_shinjuku + daimaru_tokyo (both omiyage-tagged) with their notes as hints + already-sourced counts (tokyo_banana/yoku_moku already added earlier → counts reflect that) + add-omiyage templates. Writes nothing.

## Acceptance

Per the spec: read-only `omiyage-worklist --slug` (writes 0 rows, prints tagged-POI notes verbatim + WARNING + verify steps + add-omiyage template + already-sourced count; fail-loud on unknown dest / no-tag; reject --plan-id + unknown flag; --help). No schema/pending change. Stage 3 skill documents the auto-run + verify + add-omiyage flow. All omiyage tests green serialized; 0 leaked rows.
