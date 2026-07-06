# Drill-While-Comparing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a read-only `travel compare content-depth --plan-id <drill> [--against <ref>]` command that proves an enriched drill plan reaches AND exceeds a reference trip's richness, and wire it into the Stage-3 loop-until-BETTER drill workflow — with the deployed dashboard page as the final acceptance gate.

**Architecture:** New self-contained module `compare_content_depth.rs` (read-only: `db::connect_read`, no audit triad), dispatched from `main.rs` alongside the existing `compare trips|dates|true-cost` arms. It runs a **quality-gated** per-day depth query (a stricter cousin of `validate.rs::content_depth_rows`) for both the drill plan and a reference plan (default `okinawa-2026`), renders a plain-text side-by-side table, and prints a per-axis verdict (SHORT / ALIGNED / BETTER). Docs + Stage-3 skill wire it into the drill loop.

**Tech Stack:** Rust, libsql (Turso), real-Turso integration tests on the `common::` harness.

## Global Constraints

- **Agent-first, NO JSON to stdout.** Output is plain text only. No `--json`, no machine mode. (CLAUDE.md)
- **Read-only.** `crate::db::connect_read()`. NO `plan_events` / `operation_runs` / `plans.version`. It mutates nothing.
- **plan_id hyphenated, destination underscored** — derive destination by `plan_id.replace('-', "_")`. Reference default: plan_id `okinawa-2026`, destination `okinawa_2026`.
- **Quality gates (anti-padding):** meals count only `session_type IN ('noon','evening') AND TRIM(meal) <> ''`; routes count only `duration_min IS NOT NULL AND duration_min > 0`.
- **ZH coverage = WEIGHTED ratio** `(day_zh + sess_zh) / (day_all + sess_all)`, integer percent (floor). NOT average-of-ratios. okinawa ref = 22/25 = 88%.
- **Verdict:** any axis `drill < ref` → `SHORT: <axes>` (precedence). Else all `>=` & none strictly `>` → `ALIGNED`. Else (all `>=`, ≥1 strictly `>`, quality gate applied) → `BETTER`. **Exit code 0 in every case** (diagnostic).
- **Tests hermetic:** always pass explicit `--against <seeded-ref>` (never depend on live okinawa rows). One separate test checks the DEFAULT string `okinawa-2026` appears in the header when `--against` is omitted — assert header text only, don't require the reference query to return rows.
- **Behavior-lock tests** on `common::` harness (`bin`, `db_exec`, `seed_plan`, `teardown_plan`, `Guard`, `nanos`, `is_credless`); RAII `Guard` armed right after plan-id bound; run **serialized in background** (foreground timeout SIGTERMs mid-run → Guard Drop never fires → leaked Turso row).
- **Flag rejection:** unknown flags fail loud, mirroring the existing `reject_unknown_flags` convention (see `create_plan.rs` / `set_route_segment.rs`). This command owns only `--plan-id` / `--against` (not the 5 resolver flags).
- `./bin/travel` is RELEASE — `cargo build -p travel-cli --release && cp target/release/travel ../bin/travel` before any live smoke.
- Commit ONLY this plan's pathspecs (parallel session shares the index — verify `git diff --cached --name-only`). Use `git commit -F <file>` for multi-line bodies (backticks in `-m` get shell-substituted).

---

### Task 1: Module scaffold + dispatch + `--help`

**Files:**
- Modify: `rust/crates/travel-cli/src/main.rs` (add `mod compare_content_depth;` + a dispatch arm near lines 115-133; optionally extend `print_usage()`)
- Create: `rust/crates/travel-cli/src/compare_content_depth.rs`
- Test: `rust/crates/travel-cli/tests/compare_content_depth_behavior_lock.rs`

**Interfaces:**
- Produces: `pub async fn run(rest: &[String]) -> Result<(), String>` and `pub struct ContentDepthArgs { pub plan_id: String, pub against: String }` with `pub fn parse(rest: &[String]) -> Result<ContentDepthArgs, String>`.

- [ ] **Step 1: Write the failing test** (`compare content-depth --help` prints usage)

```rust
mod common;
use common::bin;
use std::process::Command;

#[test]
fn help_prints_usage() {
    let out = Command::new(bin()).args(["compare", "content-depth", "--help"]).output().unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("Usage:"), "stdout: {s}");
    assert!(s.contains("travel compare content-depth --plan-id"), "stdout: {s}");
    assert!(s.contains("okinawa-2026"), "help should name the default reference; stdout: {s}");
}
```

- [ ] **Step 2: Run the test — expect FAIL** (routed as unknown / no usage)

Run: `cd rust && cargo test -p travel-cli --test compare_content_depth_behavior_lock help_prints_usage -- --test-threads=1`
Expected: FAIL.

- [ ] **Step 3: Minimal implementation**

In `main.rs`, add `mod compare_content_depth;` with the other `mod` lines, and a dispatch arm beside the compare siblings:

```rust
[group, sub, rest @ ..] if group == "compare" && sub == "content-depth" => {
    if rest.iter().any(|a| a == "--help" || a == "-h") {
        println!("Usage:\n  travel compare content-depth --plan-id <drill> [--against <ref>]\n  (--against default: okinawa-2026; read-only depth oracle for the drill loop)");
        return Ok(());
    }
    compare_content_depth::run(rest).await
}
```

In `compare_content_depth.rs`, stub:

```rust
use crate::db;
use libsql::params;

pub struct ContentDepthArgs { pub plan_id: String, pub against: String }

pub async fn run(_rest: &[String]) -> Result<(), String> { Ok(()) }
```

- [ ] **Step 4: Run the test — expect PASS**

Run: `cd rust && cargo test -p travel-cli --test compare_content_depth_behavior_lock help_prints_usage -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/travel-cli/src/main.rs rust/crates/travel-cli/src/compare_content_depth.rs rust/crates/travel-cli/tests/compare_content_depth_behavior_lock.rs
git commit -F <commit-msg-file>   # "feat(compare): scaffold compare content-depth + help"
```

---

### Task 2: Arg parse + destination resolution + flag rejection

**Files:**
- Modify: `rust/crates/travel-cli/src/compare_content_depth.rs`
- Test: `rust/crates/travel-cli/tests/compare_content_depth_behavior_lock.rs`

**Interfaces:**
- Consumes: `ContentDepthArgs` from Task 1.
- Produces: `ContentDepthArgs::parse`; `fn destination_for(plan_id: &str) -> String { plan_id.replace('-', "_") }`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn missing_plan_id_fails() {
    let out = Command::new(bin()).args(["compare", "content-depth", "--against", "okinawa-2026"]).output().unwrap();
    assert!(!out.status.success());
    let s = String::from_utf8_lossy(&out.stderr);
    assert!(s.contains("--plan-id"), "stderr: {s}");
}

#[test]
fn unknown_flag_fails() {
    let out = Command::new(bin()).args(["compare", "content-depth", "--plan-id", "x-2026", "--bogus"]).output().unwrap();
    assert!(!out.status.success());
    let s = String::from_utf8_lossy(&out.stderr);
    assert!(s.to_lowercase().contains("unknown flag"), "stderr: {s}");
}
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cd rust && cargo test -p travel-cli --test compare_content_depth_behavior_lock -- --test-threads=1 missing_plan_id_fails unknown_flag_fails`
Expected: FAIL (parser accepts anything / `run` is a no-op).

- [ ] **Step 3: Minimal implementation**

```rust
impl ContentDepthArgs {
    pub fn parse(rest: &[String]) -> Result<Self, String> {
        let mut plan_id: Option<String> = None;
        let mut against = "okinawa-2026".to_string();
        let mut i = 0;
        while i < rest.len() {
            match rest[i].as_str() {
                "--plan-id" => { plan_id = Some(rest.get(i + 1).ok_or("--plan-id needs a value")?.clone()); i += 2; }
                "--against" => { against = rest.get(i + 1).ok_or("--against needs a value")?.clone(); i += 2; }
                other => return Err(format!("unknown flag for compare content-depth: {other}")),
            }
        }
        Ok(Self { plan_id: plan_id.ok_or("compare content-depth requires --plan-id <drill>")?, against })
    }
}

fn destination_for(plan_id: &str) -> String { plan_id.replace('-', "_") }
```

Wire `run` to parse (still no query yet):

```rust
pub async fn run(rest: &[String]) -> Result<(), String> {
    let args = ContentDepthArgs::parse(rest)?;
    let _ = (&args.plan_id, &args.against);
    Ok(())
}
```

- [ ] **Step 4: Run — expect PASS**

Run: `cd rust && cargo test -p travel-cli --test compare_content_depth_behavior_lock -- --test-threads=1 missing_plan_id_fails unknown_flag_fails`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/travel-cli/src/compare_content_depth.rs rust/crates/travel-cli/tests/compare_content_depth_behavior_lock.rs
git commit -F <commit-msg-file>   # "feat(compare): content-depth arg parse + flag rejection"
```

---

### Task 3: Quality-gated single-plan depth query

**Files:**
- Modify: `rust/crates/travel-cli/src/compare_content_depth.rs`
- Test: `rust/crates/travel-cli/tests/compare_content_depth_behavior_lock.rs`

**Interfaces:**
- Produces: `struct DepthRow { day_number: i64, day_type: String, activities: i64, meals: i64, routes: i64 }` and `async fn depth_rows(conn, plan_id, destination) -> Result<Vec<DepthRow>, String>`.

- [ ] **Step 1: Write the failing test** (seed rows incl. anti-padding; assert gated totals)

```rust
mod common;                       // (already at top of file)
use common::{db_exec, seed_plan, teardown_plan, nanos, Guard, is_credless};

#[test]
fn gated_query_excludes_blank_meals_and_metadataless_routes() {
    if is_credless() { return; }
    let n = nanos();
    let plan = format!("test-cdepth-drill-{n}");
    let dest = plan.replace('-', "_");
    seed_plan(&plan, &dest, "2026-11-01", "2026-11-01");   // 1 day is enough
    let _g = Guard::new({ let (p, d) = (plan.clone(), dest.clone()); move || teardown_plan(&p, &d) });

    db_exec(&format!("INSERT INTO days (plan_id,destination,day_number,day_type) VALUES ('{plan}','{dest}',1,'full')"));
    // 3 activities
    for i in 0..3 { db_exec(&format!("INSERT INTO activities (plan_id,destination,day_number,session_type,sort_order,activity) VALUES ('{plan}','{dest}',1,'morning',{i},'act{i}')")); }
    // 2 real meals + 1 blank meal (must NOT count)
    db_exec(&format!("INSERT INTO session_meals (plan_id,destination,day_number,session_type,sort_order,meal,source) VALUES ('{plan}','{dest}',1,'noon',0,'Real lunch','ai_recommended')"));
    db_exec(&format!("INSERT INTO session_meals (plan_id,destination,day_number,session_type,sort_order,meal,source) VALUES ('{plan}','{dest}',1,'evening',0,'Real dinner','ai_recommended')"));
    db_exec(&format!("INSERT INTO session_meals (plan_id,destination,day_number,session_type,sort_order,meal,source) VALUES ('{plan}','{dest}',1,'noon',1,'   ','ai_recommended')"));
    // 2 routes with metadata + 1 NULL + 1 zero (only 2 count)
    db_exec(&format!("INSERT INTO day_route_segments (plan_id,destination,day_number,sort_order,from_place,to_place,mode,duration_min,source) VALUES ('{plan}','{dest}',1,0,'A','B','walk',10,'ai_recommended')"));
    db_exec(&format!("INSERT INTO day_route_segments (plan_id,destination,day_number,sort_order,from_place,to_place,mode,duration_min,source) VALUES ('{plan}','{dest}',1,1,'B','C','train',15,'ai_recommended')"));
    db_exec(&format!("INSERT INTO day_route_segments (plan_id,destination,day_number,sort_order,from_place,to_place,mode,duration_min,source) VALUES ('{plan}','{dest}',1,2,'C','D','walk',NULL,'ai_recommended')"));
    db_exec(&format!("INSERT INTO day_route_segments (plan_id,destination,day_number,sort_order,from_place,to_place,mode,duration_min,source) VALUES ('{plan}','{dest}',1,3,'D','E','walk',0,'ai_recommended')"));

    // compare drill vs itself: totals must read acts=3, meals=2, routes=2
    let out = Command::new(bin()).args(["compare","content-depth","--plan-id",&plan,"--against",&plan]).output().unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("1/2/2") || s.contains("3/2/2"), "gated per-day a/m/r wrong; stdout: {s}");
    assert!(s.contains("activities") && s.contains("meals") && s.contains("routes"), "stdout: {s}");
}
```

(Confirm the exact `activities` insert column set against `scripts/schema.sql` — the activities table
requires `session_type,sort_order,activity`; adjust the INSERT if the schema differs. The per-day
render token is `activities/meals/routes` = `3/2/2`.)

- [ ] **Step 2: Run — expect FAIL** (no query yet)

Run in background: `cd rust && cargo test -p travel-cli --test compare_content_depth_behavior_lock gated_query -- --test-threads=1`
Expected: FAIL.

- [ ] **Step 3: Minimal implementation**

```rust
struct DepthRow { day_number: i64, day_type: String, activities: i64, meals: i64, routes: i64 }

async fn depth_rows(conn: &libsql::Connection, plan_id: &str, destination: &str) -> Result<Vec<DepthRow>, String> {
    let sql = "WITH day_rows AS (SELECT day_number, day_type FROM days WHERE plan_id=?1 AND destination=?2),
      a AS (SELECT day_number, COUNT(*) n FROM activities WHERE plan_id=?1 AND destination=?2 GROUP BY day_number),
      m AS (SELECT day_number, COUNT(*) n FROM session_meals WHERE plan_id=?1 AND destination=?2 AND session_type IN ('noon','evening') AND TRIM(meal) <> '' GROUP BY day_number),
      r AS (SELECT day_number, COUNT(*) n FROM day_route_segments WHERE plan_id=?1 AND destination=?2 AND duration_min IS NOT NULL AND duration_min > 0 GROUP BY day_number)
      SELECT d.day_number, d.day_type, COALESCE(a.n,0), COALESCE(m.n,0), COALESCE(r.n,0)
      FROM day_rows d LEFT JOIN a ON a.day_number=d.day_number LEFT JOIN m ON m.day_number=d.day_number LEFT JOIN r ON r.day_number=d.day_number
      ORDER BY d.day_number";
    let mut rows = conn.query(sql, params![plan_id.to_string(), destination.to_string()]).await.map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        out.push(DepthRow {
            day_number: row.get(0).map_err(|e| e.to_string())?,
            day_type: row.get(1).map_err(|e| e.to_string())?,
            activities: row.get(2).unwrap_or(0),
            meals: row.get(3).unwrap_or(0),
            routes: row.get(4).unwrap_or(0),
        });
    }
    Ok(out)
}
```

Have `run` fetch drill rows and print a minimal per-day line (`day_number/day_type <a>/<m>/<r>`) so the
test's substring assertions can pass; the full table comes in Task 6.

- [ ] **Step 4: Run — expect PASS**

Run in background: `cd rust && cargo test -p travel-cli --test compare_content_depth_behavior_lock gated_query -- --test-threads=1`
Expected: PASS. Blank meal + NULL/zero routes excluded.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/travel-cli/src/compare_content_depth.rs rust/crates/travel-cli/tests/compare_content_depth_behavior_lock.rs
git commit -F <commit-msg-file>   # "feat(compare): quality-gated content-depth query"
```

---

### Task 4: ZH coverage ratio (weighted)

**Files:**
- Modify: `rust/crates/travel-cli/src/compare_content_depth.rs`
- Test: `rust/crates/travel-cli/tests/compare_content_depth_behavior_lock.rs`

**Interfaces:**
- Produces: `async fn zh_coverage_pct(conn, plan_id, destination) -> Result<i64, String>` returning floor percent of `(day_zh + sess_zh) / (day_all + sess_all)`; returns 0 when denominator is 0.

- [ ] **Step 1: Write the failing test** (lock 22/25 = 88%, weighted not avg)

```rust
#[test]
fn zh_coverage_is_weighted_not_avg() {
    if is_credless() { return; }
    let n = nanos();
    let plan = format!("test-cdepth-zh-{n}");
    let dest = plan.replace('-', "_");
    seed_plan(&plan, &dest, "2026-11-01", "2026-11-05");
    let _g = Guard::new({ let (p,d)=(plan.clone(),dest.clone()); move || teardown_plan(&p,&d) });
    // 5 days, all theme_zh set -> 5/5
    for day in 1..=5 { db_exec(&format!("INSERT INTO days (plan_id,destination,day_number,day_type,theme_zh) VALUES ('{plan}','{dest}',{day},'full','主題{day}')")); }
    // 20 sessions, 17 with focus_zh -> 17/20
    let mut filled = 0;
    for day in 1..=5 { for st in ["morning","noon","afternoon","evening"] {
        let zh = if filled < 17 { filled += 1; "'焦點'" } else { "NULL" };
        db_exec(&format!("INSERT INTO timesofday (plan_id,destination,day_number,session_type,focus_zh) VALUES ('{plan}','{dest}',{day},'{st}',{zh})"));
    }}
    // weighted = (5+17)/(5+20) = 22/25 = 88%  (avg-of-ratios would be 92%)
    let out = Command::new(bin()).args(["compare","content-depth","--plan-id",&plan,"--against",&plan]).output().unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("88%"), "expected weighted 88%, not 92%; stdout: {s}");
    assert!(!s.contains("92%"), "must not use avg-of-ratios (92%); stdout: {s}");
}
```

- [ ] **Step 2: Run — expect FAIL**

Run in background: `cd rust && cargo test -p travel-cli --test compare_content_depth_behavior_lock zh_coverage -- --test-threads=1`
Expected: FAIL (no ZH line / wrong formula).

- [ ] **Step 3: Minimal implementation**

```rust
async fn zh_coverage_pct(conn: &libsql::Connection, plan_id: &str, destination: &str) -> Result<i64, String> {
    let sql = "SELECT
        (SELECT COUNT(*) FROM days WHERE plan_id=?1 AND destination=?2 AND TRIM(COALESCE(theme_zh,'')) <> '')
      + (SELECT COUNT(*) FROM timesofday WHERE plan_id=?1 AND destination=?2 AND TRIM(COALESCE(focus_zh,'')) <> '') AS num,
        (SELECT COUNT(*) FROM days WHERE plan_id=?1 AND destination=?2)
      + (SELECT COUNT(*) FROM timesofday WHERE plan_id=?1 AND destination=?2) AS den";
    let mut rows = conn.query(sql, params![plan_id.to_string(), destination.to_string()]).await.map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let num: i64 = row.get(0).unwrap_or(0);
        let den: i64 = row.get(1).unwrap_or(0);
        if den == 0 { return Ok(0); }
        return Ok((num * 100) / den);   // floor
    }
    Ok(0)
}
```

Print a `ZH coverage <pct>%` line in `run`.

- [ ] **Step 4: Run — expect PASS**

Run in background: `cd rust && cargo test -p travel-cli --test compare_content_depth_behavior_lock zh_coverage -- --test-threads=1`
Expected: PASS. `88%` present, `92%` absent.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/travel-cli/src/compare_content_depth.rs rust/crates/travel-cli/tests/compare_content_depth_behavior_lock.rs
git commit -F <commit-msg-file>   # "feat(compare): weighted ZH coverage axis"
```

---

### Task 5: Two-plan verdict logic (SHORT / ALIGNED / BETTER)

**Files:**
- Modify: `rust/crates/travel-cli/src/compare_content_depth.rs`
- Test: `rust/crates/travel-cli/tests/compare_content_depth_behavior_lock.rs`

**Interfaces:**
- Produces: `struct Totals { activities: i64, meals: i64, routes: i64, zh: i64 }`; `fn verdict(drill: &Totals, refr: &Totals) -> String` returning a line starting `VERDICT: SHORT: <axes>` | `VERDICT: ALIGNED` | `VERDICT: BETTER — ...`.

- [ ] **Step 1: Write the failing tests** (SHORT / ALIGNED / BETTER + anti-padding)

Add helper to seed a plan with N days each carrying `acts`/`meals`/`routes`/full-ZH so totals are
predictable, then:

```rust
// thin drill vs rich ref -> SHORT: meals (and/or others)
#[test] fn verdict_short() { /* seed ref richer on meals; assert stdout contains "VERDICT: SHORT:" and "meals" */ }

// exact tie everywhere -> ALIGNED
#[test] fn verdict_aligned() { /* seed drill == ref on all axes incl zh; assert "VERDICT: ALIGNED" */ }

// drill >= ref, strictly > on activities -> BETTER
#[test] fn verdict_better() { /* seed drill one extra activity; assert "VERDICT: BETTER" */ }

// anti-padding: drill has MORE raw route rows but FEWER duration_min>0 routes than ref -> NOT BETTER
#[test]
fn verdict_antipadding_routes() {
    // ref: 3 routes all duration_min>0 ; drill: 5 route rows but only 2 with duration_min>0
    // -> routes axis drill(2) < ref(3) -> must contain "VERDICT: SHORT" and "routes", must NOT contain "BETTER"
}
```

Every test: `if is_credless() { return; }`, `Guard` armed right after plan-id, exit code success
asserted for SHORT too (`out.status.success()`), run serialized in background.

- [ ] **Step 2: Run — expect FAIL**

Run in background: `cd rust && cargo test -p travel-cli --test compare_content_depth_behavior_lock verdict -- --test-threads=1`
Expected: FAIL.

- [ ] **Step 3: Minimal implementation**

```rust
struct Totals { activities: i64, meals: i64, routes: i64, zh: i64 }

fn totals_of(rows: &[DepthRow], zh: i64) -> Totals {
    Totals {
        activities: rows.iter().map(|r| r.activities).sum(),
        meals: rows.iter().map(|r| r.meals).sum(),
        routes: rows.iter().map(|r| r.routes).sum(),
        zh,
    }
}

fn verdict(drill: &Totals, refr: &Totals) -> String {
    let axes: [(&str, i64, i64); 4] = [
        ("activities", drill.activities, refr.activities),
        ("meals", drill.meals, refr.meals),
        ("routes", drill.routes, refr.routes),
        ("ZH", drill.zh, refr.zh),
    ];
    let short: Vec<&str> = axes.iter().filter(|(_, d, r)| d < r).map(|(n, _, _)| *n).collect();
    if !short.is_empty() {
        return format!("VERDICT: SHORT: {}", short.join(", "));
    }
    let strictly_greater = axes.iter().filter(|(_, d, r)| d > r).count();
    if strictly_greater == 0 {
        return "VERDICT: ALIGNED — every axis meets the reference exactly".to_string();
    }
    format!("VERDICT: BETTER — all axes >= reference, {strictly_greater} strictly greater, quality gate PASS")
}
```

Wire `run`: fetch drill + ref `depth_rows` and `zh_coverage_pct`, build both `Totals`, print the verdict line last. Exit `Ok(())` regardless of verdict (exit 0).

- [ ] **Step 4: Run — expect PASS**

Run in background: `cd rust && cargo test -p travel-cli --test compare_content_depth_behavior_lock verdict -- --test-threads=1`
Expected: PASS (all four verdict tests; anti-padding stays SHORT/not-BETTER).

- [ ] **Step 5: Commit**

```bash
git add rust/crates/travel-cli/src/compare_content_depth.rs rust/crates/travel-cli/tests/compare_content_depth_behavior_lock.rs
git commit -F <commit-msg-file>   # "feat(compare): content-depth verdict logic (SHORT/ALIGNED/BETTER)"
```

---

### Task 6: Plain-text side-by-side table rendering

**Files:**
- Modify: `rust/crates/travel-cli/src/compare_content_depth.rs`
- Test: `rust/crates/travel-cli/tests/compare_content_depth_behavior_lock.rs`

**Interfaces:**
- Consumes: `DepthRow`, `Totals`, `verdict`, `zh_coverage_pct`.
- Produces: the final rendered output (header + `per-day:` block + `totals:` block + verdict).

- [ ] **Step 1: Write the failing test** (full output shape)

```rust
#[test]
fn renders_header_perday_and_totals() {
    if is_credless() { return; }
    // seed a small drill + explicit ref; assert:
    //  - header:  "CONTENT DEPTH" and "<drill>" and "<ref>" and "(reference)"
    //  - "per-day:" present
    //  - column labels: "DRILL" and "REF"
    //  - "totals:" present with lines: "activities", "meals (real)", "routes (w/ metadata)", "ZH coverage"
    //  - a "VERDICT:" line
    // Prefer substring assertions over byte-perfect spacing.
}
```

- [ ] **Step 2: Run — expect FAIL** (output incomplete)

Run in background: `cd rust && cargo test -p travel-cli --test compare_content_depth_behavior_lock renders_ -- --test-threads=1`
Expected: FAIL.

- [ ] **Step 3: Minimal implementation**

Render, joining drill+ref rows by `day_number` (ref-missing day → `0/0/0`; iterate the union of day numbers, ordered):

```
CONTENT DEPTH — {drill}  vs  {against} (reference)

per-day:
  day  type        DRILL(a/m/r)   REF(a/m/r)
  {day:<4} {type:<11} {da}/{dm}/{dr:<10} {ra}/{rm}/{rr}
  ...

totals:
                          DRILL   REF    Δ       verdict
  activities              {da}    {ra}   {sign}  {>=|<}
  meals (real)            {dm}    {rm}   ...
  routes (w/ metadata)    {dr}    {rr}   ...
  ZH coverage             {dzh}%  {rzh}% {±pp}   ...
  ------------------------------------------------------
  {verdict line}
```

Use small pad helpers; `Δ` as `+N`/`-N`/`0` (and `+Npp`/`-Npp` for ZH). Per-axis `verdict` cell: `>=` if `drill>=ref` else `<`. Keep plain text; no JSON.

- [ ] **Step 4: Run — expect PASS**

Run in background: `cd rust && cargo test -p travel-cli --test compare_content_depth_behavior_lock renders_ -- --test-threads=1`
Then the whole file: `cd rust && cargo test -p travel-cli --test compare_content_depth_behavior_lock -- --test-threads=1`
Expected: PASS (all).

- [ ] **Step 5: Build release + live smoke**

```bash
cd rust && cargo build -p travel-cli --release && cp target/release/travel ../bin/travel && cd ..
export TRAVEL_TURSO_URL=$(grep '^TURSO_URL=' .env | cut -d= -f2-); export TRAVEL_TURSO_READ_TOKEN=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-); export TRAVEL_TURSO_WRITE_TOKEN=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-)
./bin/travel compare content-depth --plan-id kyoto-confirm-2026 --against okinawa-2026   # expect SHORT (thin drill)
```

- [ ] **Step 6: Commit**

```bash
git add rust/crates/travel-cli/src/compare_content_depth.rs rust/crates/travel-cli/tests/compare_content_depth_behavior_lock.rs
git commit -F <commit-msg-file>   # "feat(compare): render content-depth side-by-side table"
```

---

### Task 7: Docs + Stage-3 skill wiring (loop-until-BETTER)

**Files:**
- Modify: `src/skills/stage3-expand-itinerary/SKILL.md` (bump to v1.3.0; add the loop-until-BETTER callout)
- Modify: `docs/reference/CLI.md` (add under the comparison-views section)
- Modify: `CLAUDE.md` (Skill Decision Tree + CLI Quick Reference comparison lines)

**Interfaces:** docs only, no Rust behavior change.

- [ ] **Step 1: Grep to confirm absence (pre-check)**

Run: `rg "compare content-depth" src/skills/stage3-expand-itinerary/SKILL.md docs/reference/CLI.md CLAUDE.md`
Expected: no hits.

- [ ] **Step 2: Edit the Stage-3 skill** — after step 7 (validate + content-depth), add a loop-until-BETTER callout:

> After the content-depth WARNs are addressed, run the depth ORACLE against a known-good reference:
> `./bin/travel compare content-depth --plan-id <plan_id> [--against okinawa-2026]`.
> Treat the `SHORT: <axes>` line as the enrichment worklist — enrich the named axes (agent-first meals on the SHORT days, re-run `derive-routes --day N` for routes), then re-compare. Repeat until `VERDICT: BETTER`.
> **This is a mid-loop oracle, NOT final acceptance.** The final gate is the deployed dashboard page reviewed side by side with the reference (Stage 4).

- [ ] **Step 3: Edit `docs/reference/CLI.md`** — add:

```
./bin/travel compare content-depth --plan-id <drill> [--against okinawa-2026]   # read-only depth oracle: per-axis drill-vs-reference verdict (SHORT/ALIGNED/BETTER); quality-gated (real meals, routes w/ metadata, weighted ZH)
```

- [ ] **Step 4: Edit `CLAUDE.md`** — add a Skill Decision Tree row:

```
"is the drill/plan rich enough" / "compare depth to a real trip"  → ./bin/travel compare content-depth --plan-id <id> [--against okinawa-2026]  (read-only oracle; loop-until-BETTER; web page is final gate)
```

and add the command to the comparison list in CLI Quick Reference.

- [ ] **Step 5: Verify + commit**

```bash
rg "compare content-depth" src/skills/stage3-expand-itinerary/SKILL.md docs/reference/CLI.md CLAUDE.md   # expect hits in all three
export ... (turso env) ; ./bin/travel validate data    # pre-commit gate: 0/0/0
git add src/skills/stage3-expand-itinerary/SKILL.md docs/reference/CLI.md CLAUDE.md
git commit -F <commit-msg-file>   # "docs(flow): wire compare content-depth into the Stage-3 drill loop"
```

---

## Final Verification

```bash
cd rust && cargo test -p travel-cli --test compare_content_depth_behavior_lock -- --test-threads=1   # (run in BACKGROUND) all green
```

Then the drill-while-comparing run itself (separate, human-in-loop):
1. Fresh drill plan → enriched Stage-3 flow → `compare content-depth` loop until `BETTER`.
2. `validate publish --plan-id <drill>` → 0 blockers.
3. **Yang deploys** `-rs` (`cd workers/trip-dashboard-rs && unset CLOUDFLARE_API_TOKEN && npx wrangler deploy`).
4. `share-token` → review drill vs okinawa live, side by side (**final acceptance**).
5. `share-token deactivate <tok>`.

Expected end properties: read-only; exit 0 for SHORT/ALIGNED/BETTER; meals gated on non-empty; routes gated on `duration_min>0`; ZH weighted `(day_zh+sess_zh)/(day_all+sess_all)`; plain text, no JSON.
