# content-depth ZH-gate + de-hardcode hints Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `compare content-depth` treat ZH as a completeness GATE (aligned exactly with `validate.rs`'s existing `missing_day_zh`/`missing_session_zh`), not a 4th comparable depth axis — so an honest short trip with empty arrival/departure sessions is not falsely SHORT on ZH and cannot be pushed to fabricate ZH. Plus de-hardcode 4 Okinawa place-name example strings in hint text (G1).

**Architecture:** G2 lives entirely in `compare_content_depth.rs`: replace `zh_coverage_pct` (returns a %) with a gate that returns `(num, den)` over content-bearing days+sessions using the SAME OR-chain eligibility as `validate.rs`; drop ZH from the 4-axis `verdict` array (→ 3 depth axes) and add a pre-verdict gate check; move ZH out of the `Totals` render into a two-row `gates:` block; warn-and-continue if the reference gate fails. G1 is 4 one-line string edits across 3 files.

**Tech Stack:** Rust (travel-cli read-only compare command), real-Turso integration tests on `tests/common/mod.rs` via the `compare_content_depth_behavior_lock.rs` harness (`run_or_skip` — credless-skips).

## Global Constraints

- **Agent-first plain text** — stdout, no JSON.
- **Fail loud / no cheat** — a real depth deficit or gate failure must surface; the gate must NOT let an agent pass by fabricating ZH on empty sessions.
- **Read-only command** — no mutation, no audit triad, no DB writes.
- **EXACT validator alignment (the load-bearing rule).** ZH eligibility MUST match `validate.rs` verbatim:
  - day eligible ⟺ `EXISTS activities(by day) OR EXISTS session_meals(by day) OR EXISTS day_route_segments(by day)` (validate.rs:1493-1507).
  - session eligible ⟺ `EXISTS activities(by day,session) OR EXISTS session_meals(by day,session) OR transit_notes non-blank OR transit_notes_zh non-blank` (validate.rs:1521-1536).
  - day translated ⟺ `theme_zh` non-blank; session translated ⟺ `focus_zh` non-blank OR `transit_notes_zh` non-blank (validate.rs:1537-1538).
  - "non-blank" = `NULLIF(TRIM(COALESCE(col,'')),'') IS NOT NULL` (verbatim).
  - **eligibility keys on content existence (activities/meals/routes/transit), NEVER on the ZH column itself** — else a missing translation drops out of the denominator and wrongly PASSes.
  - gate PASS ⟺ `num == den`; `0/0` (no eligible slot) = vacuous PASS, printed `0/0 PASS`.
- **Verdict:** SHORT = any depth axis < ref OR drill gate FAIL (list deficits in activities/meals/routes order, then append `ZH-gate` if gate FAIL). BETTER = no SHORT AND ≥1 depth axis strictly >. ALIGNED = no SHORT AND all 3 depth axes == ref.
- **Reference gate FAIL = warn-and-continue:** print `⚠ reference ZH gate FAIL (N/M); depth comparison continues; drill must independently PASS` to stdout, exit 0, do NOT abort, do NOT change the verdict, do NOT lower the drill requirement.
- **Spec:** `docs/superpowers/specs/2026-07-12-content-depth-zh-gate-and-hint-hardcode.md` — read it; it is authoritative.
- **Pipeline:** Grok 4.5 implements task-by-task; Claude reviews every line + corroborates vs source + verifies serialized. Commit explicit pathspecs only.

---

## File Structure

- `rust/crates/travel-cli/src/compare_content_depth.rs` — replace `zh_coverage_pct` with `zh_gate` (returns `(i64,i64)`); update `Totals`/`totals_of`; change `verdict` from 4 axes to 3 + gate; update render (`totals` block loses ZH row; add `gates:` block + reference-gate warning). (G2)
- `rust/crates/travel-cli/tests/compare_content_depth_behavior_lock.rs` — rewrite `zh_coverage_is_weighted_not_avg`; fix `renders_header_perday_and_totals` (:446); add the new gate lock cases. (G2 tests)
- `rust/crates/travel-cli/src/set_tod.rs:285`, `set_route_segment.rs:60`, `validate_itinerary.rs:360`, `validate_itinerary.rs:411` — G1 string edits.
- `rust/crates/travel-cli/tests/cli_help_parity.rs` OR a small new test — G1 lock (optional; the strings are cosmetic — a grep-based assertion that the 4 files no longer contain the Okinawa tokens is enough).
- `docs/reference/CLI.md`, `CLAUDE.md` — doc updates (Task 4).

---

## Task 1 (commit 1) — G2 core: ZH gate replaces the ZH axis

**Files:**
- Modify: `rust/crates/travel-cli/src/compare_content_depth.rs` (`zh_coverage_pct`→`zh_gate`, `Totals`, `totals_of`, `verdict`, render fn)
- Test: `rust/crates/travel-cli/tests/compare_content_depth_behavior_lock.rs`

**Interfaces:**
- Produces: `async fn zh_gate(conn, plan_id, destination) -> Result<(i64, i64), String>` returning `(translated_eligible, total_eligible)` over content-bearing days+sessions. `Totals` loses the `zh: i64` field (or repurposes — see below). `verdict(drill_depth, ref_depth, drill_gate_pass, ref_gate_pass)` signature changes.

- [ ] **Step 1: Write the failing tests**

Rewrite `zh_coverage_is_weighted_not_avg` and add the gate cases. Study the existing helpers `seed_depth_counts` (`:168`) and `seed_antipadding_drill` (`:211`) — reuse them. The key seed facts: `seed_depth_counts(full_zh=true)` sets day theme_zh + all 4 session focus_zh, activities in `morning`, meals in noon/evening, routes unscoped → eligible sessions = morning(activity)+noon+evening(meals) = 3, all translated → gate PASS; `afternoon` has no content → not eligible.

Add these tests (use the file's `run_or_skip`, `db_exec`, `nanos`, `Guard`, `teardown_plan` conventions — copy an existing test's scaffold):

```rust
// Gate PASS: every content-bearing session translated (empty scaffold session ignored).
#[test]
fn zh_gate_passes_when_content_sessions_translated() {
    let n = nanos(); let plan = format!("cdz-pass-{n}"); let dest = format!("cdz_pass_{n}");
    let _g = Guard::new({ let (p,d)=(plan.clone(),dest.clone()); move || teardown_plan(&p,&d) });
    // day with 1 activity (morning) + theme_zh + focus_zh on morning; afternoon empty + no ZH.
    if db_exec(&format!(
        "INSERT INTO days (plan_id,destination,day_number,date,day_type,status,theme_zh,updated_at) VALUES ('{plan}','{dest}',1,'2026-11-01','full','draft','主題','2020-01-01 00:00:00');\
         INSERT INTO activities (id,plan_id,destination,day_number,session_type,sort_order,title,updated_at) VALUES ('{n}-a','{plan}','{dest}',1,'morning',0,'act','2020-01-01 00:00:00');\
         INSERT INTO timesofday (plan_id,destination,day_number,session_type,focus_zh) VALUES ('{plan}','{dest}',1,'morning','焦點'),('{plan}','{dest}',1,'afternoon',NULL);"
    )).is_none() { eprintln!("credless"); return; }
    // Compare against itself → depth ALIGNED, gate PASS both → ALIGNED, and gates block shows PASS.
    let Some(out) = run_or_skip(&["compare","content-depth","--plan-id",&plan,"--against",&plan]) else { return };
    assert!(out.contains("PASS"), "expected gate PASS; out={out}");
    assert!(!out.contains("ZH coverage"), "ZH must be a gate, not a totals row; out={out}");
    assert!(out.contains("ZH slot completeness"), "gate label; out={out}");
}

// Gate FAIL forces SHORT even when depth axes are equal: a content session missing ZH.
#[test]
fn zh_gate_fail_forces_short() {
    let n = nanos(); let plan = format!("cdz-fail-{n}"); let dest = format!("cdz_fail_{n}");
    let _g = Guard::new({ let (p,d)=(plan.clone(),dest.clone()); move || teardown_plan(&p,&d) });
    // morning has an activity but NO focus_zh → eligible + untranslated → gate FAIL.
    if db_exec(&format!(
        "INSERT INTO days (plan_id,destination,day_number,date,day_type,status,theme_zh,updated_at) VALUES ('{plan}','{dest}',1,'2026-11-01','full','draft','主題','2020-01-01 00:00:00');\
         INSERT INTO activities (id,plan_id,destination,day_number,session_type,sort_order,title,updated_at) VALUES ('{n}-a','{plan}','{dest}',1,'morning',0,'act','2020-01-01 00:00:00');\
         INSERT INTO timesofday (plan_id,destination,day_number,session_type,focus_zh) VALUES ('{plan}','{dest}',1,'morning',NULL);"
    )).is_none() { eprintln!("credless"); return; }
    let Some(out) = run_or_skip(&["compare","content-depth","--plan-id",&plan,"--against",&plan]) else { return };
    assert!(out.contains("FAIL"), "gate should FAIL; out={out}");
    assert!(out.contains("SHORT") && out.contains("ZH-gate"), "gate FAIL → SHORT: ZH-gate; out={out}");
}

// transit_notes_zh alone satisfies "translated"; a meal-only session is eligible (OR-chain).
#[test]
fn zh_gate_transit_zh_and_meal_only_eligibility() {
    let n = nanos(); let plan = format!("cdz-tz-{n}"); let dest = format!("cdz_tz_{n}");
    let _g = Guard::new({ let (p,d)=(plan.clone(),dest.clone()); move || teardown_plan(&p,&d) });
    // noon: meal only (eligible via meal), translated via transit_notes_zh (no focus_zh). morning: transit_notes only, focus_zh set.
    if db_exec(&format!(
        "INSERT INTO days (plan_id,destination,day_number,date,day_type,status,theme_zh,updated_at) VALUES ('{plan}','{dest}',1,'2026-11-01','full','draft','主題','2020-01-01 00:00:00');\
         INSERT INTO session_meals (plan_id,destination,day_number,session_type,sort_order,meal,source) VALUES ('{plan}','{dest}',1,'noon',0,'Lunch','ai_recommended');\
         INSERT INTO timesofday (plan_id,destination,day_number,session_type,focus_zh,transit_notes,transit_notes_zh) VALUES ('{plan}','{dest}',1,'noon',NULL,NULL,'交通備註'),('{plan}','{dest}',1,'morning','焦點','x',NULL);"
    )).is_none() { eprintln!("credless"); return; }
    let Some(out) = run_or_skip(&["compare","content-depth","--plan-id",&plan,"--against",&plan]) else { return };
    // noon eligible-by-meal + translated-by-transit_zh; morning eligible-by-transit + translated-by-focus → all translated → PASS.
    assert!(out.contains("PASS"), "OR-chain eligibility + transit_zh translation → PASS; out={out}");
}
```

Also rewrite `zh_coverage_is_weighted_not_avg` → delete it (its 88%/weighted semantics no longer exist) OR repurpose its name to a gate case; simplest is to remove it and rely on the three above. If removed, ensure no other test references it.

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd rust && cargo build -p travel-cli && cargo test -p travel-cli --test compare_content_depth_behavior_lock -- --test-threads=1 --nocapture
```
Expected: the new gate tests FAIL (current output has `ZH coverage` in totals, no `gates:`/`ZH slot completeness`, no `ZH-gate` in SHORT). `zh_coverage_is_weighted_not_avg` (if kept) still asserts 88% — will conflict once implemented; that's why it's removed/rewritten.

- [ ] **Step 3: Implement `zh_gate` (replace `zh_coverage_pct`)**

In `compare_content_depth.rs`, replace `zh_coverage_pct` (:80-103) with a fn returning `(translated_eligible, total_eligible)`. The SQL must mirror `validate.rs` eligibility EXACTLY. Two independent counts (days, sessions), summed:

```rust
/// ZH slot-completeness gate, aligned verbatim with validate.rs missing_day_zh/missing_session_zh.
/// Returns (translated_eligible, total_eligible) over content-bearing days + sessions.
async fn zh_gate(
    conn: &libsql::Connection,
    plan_id: &str,
    destination: &str,
) -> Result<(i64, i64), String> {
    // Day: eligible = activities OR meals OR routes; translated = theme_zh non-blank.
    // Session: eligible = activities OR meals OR transit_notes OR transit_notes_zh;
    //          translated = focus_zh non-blank OR transit_notes_zh non-blank.
    let sql = "SELECT
      (SELECT COUNT(*) FROM days d WHERE d.plan_id=?1 AND d.destination=?2
         AND ( EXISTS(SELECT 1 FROM activities a WHERE a.plan_id=d.plan_id AND a.destination=d.destination AND a.day_number=d.day_number)
            OR EXISTS(SELECT 1 FROM session_meals m WHERE m.plan_id=d.plan_id AND m.destination=d.destination AND m.day_number=d.day_number)
            OR EXISTS(SELECT 1 FROM day_route_segments r WHERE r.plan_id=d.plan_id AND r.destination=d.destination AND r.day_number=d.day_number) )
      ) AS day_elig,
      (SELECT COUNT(*) FROM days d WHERE d.plan_id=?1 AND d.destination=?2
         AND ( EXISTS(SELECT 1 FROM activities a WHERE a.plan_id=d.plan_id AND a.destination=d.destination AND a.day_number=d.day_number)
            OR EXISTS(SELECT 1 FROM session_meals m WHERE m.plan_id=d.plan_id AND m.destination=d.destination AND m.day_number=d.day_number)
            OR EXISTS(SELECT 1 FROM day_route_segments r WHERE r.plan_id=d.plan_id AND r.destination=d.destination AND r.day_number=d.day_number) )
         AND NULLIF(TRIM(COALESCE(d.theme_zh,'')),'') IS NOT NULL
      ) AS day_tr,
      (SELECT COUNT(*) FROM timesofday t WHERE t.plan_id=?1 AND t.destination=?2
         AND ( EXISTS(SELECT 1 FROM activities a WHERE a.plan_id=t.plan_id AND a.destination=t.destination AND a.day_number=t.day_number AND a.session_type=t.session_type)
            OR EXISTS(SELECT 1 FROM session_meals m WHERE m.plan_id=t.plan_id AND m.destination=t.destination AND m.day_number=t.day_number AND m.session_type=t.session_type)
            OR NULLIF(TRIM(COALESCE(t.transit_notes,'')),'') IS NOT NULL
            OR NULLIF(TRIM(COALESCE(t.transit_notes_zh,'')),'') IS NOT NULL )
      ) AS sess_elig,
      (SELECT COUNT(*) FROM timesofday t WHERE t.plan_id=?1 AND t.destination=?2
         AND ( EXISTS(SELECT 1 FROM activities a WHERE a.plan_id=t.plan_id AND a.destination=t.destination AND a.day_number=t.day_number AND a.session_type=t.session_type)
            OR EXISTS(SELECT 1 FROM session_meals m WHERE m.plan_id=t.plan_id AND m.destination=t.destination AND m.day_number=t.day_number AND m.session_type=t.session_type)
            OR NULLIF(TRIM(COALESCE(t.transit_notes,'')),'') IS NOT NULL
            OR NULLIF(TRIM(COALESCE(t.transit_notes_zh,'')),'') IS NOT NULL )
         AND ( NULLIF(TRIM(COALESCE(t.focus_zh,'')),'') IS NOT NULL
            OR NULLIF(TRIM(COALESCE(t.transit_notes_zh,'')),'') IS NOT NULL )
      ) AS sess_tr";
    let mut rows = conn.query(sql, params![plan_id.to_string(), destination.to_string()]).await.map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let day_elig: i64 = row.get(0).unwrap_or(0);
        let day_tr: i64 = row.get(1).unwrap_or(0);
        let sess_elig: i64 = row.get(2).unwrap_or(0);
        let sess_tr: i64 = row.get(3).unwrap_or(0);
        return Ok((day_tr + sess_tr, day_elig + sess_elig));
    }
    Ok((0, 0))
}
```
(Verify column/table names against `validate.rs:1484-1540` before finalizing — they must be identical.)

- [ ] **Step 4: Update `Totals`, `verdict`, and render**

- `Totals` (`:105-118`): remove the `zh` field (or leave a `(num,den)` gate tuple — cleaner to drop `zh` from `Totals` and carry the gate `(num,den)` separately in `run`). Keep `activities/meals/routes`.
- `verdict` (`:121-141`): change `axes` array from `[…;4]` to `[…;3]` (activities/meals/routes only). Add gate params. New logic:
  ```rust
  fn verdict(drill: &Totals, refr: &Totals, drill_gate_pass: bool) -> String {
      let axes: [(&str, i64, i64); 3] = [
          ("activities", drill.activities, refr.activities),
          ("meals", drill.meals, refr.meals),
          ("routes", drill.routes, refr.routes),
      ];
      let mut short: Vec<String> = axes.iter().filter(|(_,d,r)| d<r).map(|(n,_,_)| n.to_string()).collect();
      if !drill_gate_pass { short.push("ZH-gate".to_string()); }
      if !short.is_empty() { return format!("VERDICT: SHORT: {}", short.join(", ")); }
      let strictly_greater = axes.iter().filter(|(_,d,r)| d>r).count();
      if strictly_greater == 0 {
          return "VERDICT: ALIGNED — all 3 depth axes equal reference; ZH gate PASS".to_string();
      }
      format!("VERDICT: BETTER — all 3 depth axes >= reference, {strictly_greater} strictly greater; ZH gate PASS")
  }
  ```
- Render (the fn that prints per-day + totals, around `:195-247`): drop the ZH row from `totals`; after `totals`, print the `gates:` block (two rows, drill + ref) then the reference-gate warning if ref gate FAIL, then verdict. gate PASS ⟺ `num==den`:
  ```rust
  // ... after the 3-row totals block ...
  let (dn, dd) = drill_gate; // (num, den)
  let (rn, rd) = ref_gate;
  let dp = dn == dd; let rp = rn == rd;
  println!("\ngates:");
  println!("  ZH slot completeness  drill {dn}/{dd}  {}", if dp {"PASS"} else {"FAIL"});
  println!("  ZH slot completeness  ref   {rn}/{rd}  {}", if rp {"PASS"} else {"FAIL"});
  if !rp {
      println!("⚠ reference ZH gate FAIL ({rn}/{rd}); depth comparison continues; drill must independently PASS");
  }
  println!("\n{}", verdict(&drill_totals, &ref_totals, dp));
  ```
  Thread `drill_gate`/`ref_gate` from `run` (call `zh_gate` for both plan_id/dest like the old `zh_coverage_pct` calls at `:182-183`).

- [ ] **Step 5: Build + run tests to verify they pass**

```bash
cd rust && cargo build -p travel-cli && cargo test -p travel-cli --test compare_content_depth_behavior_lock -- --test-threads=1 --nocapture
```
Expected: new gate tests PASS; `renders_header_perday_and_totals` (:446) will now FAIL because it asserts `ZH coverage` — fix it in Step 6.

- [ ] **Step 6: Fix `renders_header_perday_and_totals` (:446)**

That test asserts `ZH coverage` in the totals block. Update it to assert the new `gates:` block (`ZH slot completeness` + a `PASS`/`FAIL` row for drill and ref) and that `ZH coverage` is GONE from totals. Re-run the full file green.

- [ ] **Step 7: Commit**

```bash
cd /home/yanggf/b/travel-2026
git add rust/crates/travel-cli/src/compare_content_depth.rs rust/crates/travel-cli/tests/compare_content_depth_behavior_lock.rs
git commit -F - <<'EOF'
fix(cli): content-depth treats ZH as a completeness gate, not a depth axis (G2)

ZH coverage counted empty scaffold sessions in its denominator, so an honest
short trip (arrival-PM/departure-AM → legit empty sessions) was falsely SHORT on
ZH, and the drill loop pressured filling empties (cheating). ZH is now a
slot-completeness GATE aligned verbatim with validate.rs missing_day_zh/
missing_session_zh (eligible = activities OR meals OR routes/transit; translated
= theme_zh / focus_zh|transit_notes_zh). content-depth compares 3 depth axes
(activities/meals/routes) + a ZH gate; gate FAIL forces SHORT: ZH-gate; reference
gate FAIL warns and continues. Found in the kyoto-oct real-data redrill.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Task 2 (commit 2) — G1: de-hardcode the 4 Okinawa place-name hints

**Files:**
- Modify: `rust/crates/travel-cli/src/set_tod.rs:285`, `set_route_segment.rs:60`, `validate_itinerary.rs:360`, `validate_itinerary.rs:411`
- Test: add a grep-style lock to `compare_content_depth_behavior_lock.rs`? No — put it in a tiny focused test or `cli_help_parity.rs`. Simplest: a compile-time-free `#[test]` that reads the 4 source files and asserts the Okinawa tokens are gone. But source-file-reading tests are brittle. **Preferred:** no automated test (cosmetic strings); verify by live smoke + grep in Step 3. If a lock is wanted, assert via a runtime trigger (below).

- [ ] **Step 1: Edit the 4 strings (exact replacements from spec)**

Apply verbatim (only the example place names change; all guidance preserved):
- `set_tod.rs:285`: `e.g. "安里駅 → 赤嶺駅 → iias 沖縄豊崎"` → `e.g. "<站A> → <站B> → <地標>"`
- `set_route_segment.rs:60`: `(e.g. "赤嶺駅", "iias 沖縄豊崎")` → `(e.g. "<車站>", "<地標>")` (keep `keep both stops in the same country; use mode=transit for a rail/bus leg.` unchanged)
- `validate_itinerary.rs:360`: `(e.g. "赤嶺駅", "iias 沖縄豊崎")` → `(e.g. "<車站>", "<地標>")`
- `validate_itinerary.rs:411`: `(e.g. "安里駅 那覇")` → `(e.g. "<車站>駅 <城市>")`

- [ ] **Step 2: Verify (build + grep + live smoke)**

```bash
cd rust && cargo build -p travel-cli
# grep: the 4 source files no longer carry the Okinawa example tokens
grep -n "安里\|赤嶺\|豊崎\|iias" rust/crates/travel-cli/src/set_tod.rs rust/crates/travel-cli/src/set_route_segment.rs rust/crates/travel-cli/src/validate_itinerary.rs
```
Expected: grep returns NOTHING in those 4 files (checks.rs + tests are untouched and out of scope). Optionally trigger `set_route_segment.rs:60` live (a stop with a parenthetical) and confirm the hint now shows `<車站>`/`<地標>`.

- [ ] **Step 3: Commit**

```bash
cd /home/yanggf/b/travel-2026
git add rust/crates/travel-cli/src/set_tod.rs rust/crates/travel-cli/src/set_route_segment.rs rust/crates/travel-cli/src/validate_itinerary.rs
git commit -F - <<'EOF'
fix(cli): de-hardcode Okinawa place names in 4 stop-hint examples (G1)

Four user-facing "clean stop name" hints hardcoded Okinawa places (安里駅/赤嶺駅/
iias 沖縄豊崎/那覇) as examples, so editing a non-Okinawa plan showed Okinawa
examples. Replaced with schematic placeholders (<站A>/<車站>/<地標>/<城市>);
all rule guidance (same-country, mode=transit, no （…）notes/clock-times) kept
verbatim. checks.rs country-classification logic + regression-test place names
are out of scope. Found in the kyoto-oct real-data redrill.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Task 3 (commit 3) — docs: CLI.md update + CLAUDE.md history annotation

**Files:** `docs/reference/CLI.md`, `CLAUDE.md`

- [ ] **Step 1: Update CLI.md content-depth description**

Find the `compare content-depth` entry in `docs/reference/CLI.md` and update it: "3 depth axes (activities / real-meals / routes-with-metadata) compared vs reference + a ZH slot-completeness gate (aligned with validate publish's ZH gate); BETTER = gate PASS + all 3 depth axes ≥ ref + ≥1 strictly >." Remove any "4 axes / ZH coverage %" wording.

- [ ] **Step 2: Annotate CLAUDE.md**

Where CLAUDE.md's current text describes content-depth as a 4-axis "SHORT/ALIGNED/BETTER" oracle with a ZH % (the compare-content-depth entries), add a short inline note: "(ZH is now a completeness gate, not a 4th %-axis — see 2026-07-12 spec)." Do NOT rewrite the historical drill entries (okinawa 29/10/21/88% etc. stay as historical records of the old metric). Do NOT touch the old 2026-07-06 design/plan docs.

- [ ] **Step 3: Commit**

```bash
cd /home/yanggf/b/travel-2026
git add docs/reference/CLI.md CLAUDE.md
git commit -F - <<'EOF'
docs: content-depth now 3 depth axes + ZH gate (CLI.md + CLAUDE.md note)

Update the live CLI.md content-depth description to the new model; annotate
CLAUDE.md's current description that ZH is a completeness gate now. Historical
drill numbers (old 4-axis metric) left as-is per verify-against-committed-tree.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Live smoke (after all commits) — the real kyoto-oct plan

```bash
cd /home/yanggf/b/travel-2026
export TRAVEL_TURSO_URL=$(grep '^TURSO_URL=' .env | cut -d= -f2-)
export TRAVEL_TURSO_READ_TOKEN=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-)
export TRAVEL_TURSO_WRITE_TOKEN=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-)
./bin/travel compare content-depth --plan-id kyoto-oct-2026 --against okinawa-2026
```
Expected: 3-row totals (activities 31, meals 12, routes 22 — all > okinawa's 29/10/21), a `gates:` block showing `drill 20/20 PASS` + `ref 19/19 PASS`, and `VERDICT: BETTER — all 3 depth axes >= reference, 3 strictly greater; ZH gate PASS`. The kyoto false-SHORT-on-ZH is gone WITHOUT fabricating any ZH.

## Acceptance

- G2: content-depth prints 3 depth axes + a two-row ZH gate; gate eligibility == validate.rs verbatim (activities OR meals OR routes/transit); gate FAIL → `SHORT: …, ZH-gate`; reference gate FAIL warns + continues + exit 0; `0/0` = PASS. kyoto-oct → BETTER, gate PASS, no fabricated ZH.
- G1: the 4 hint strings carry schematic placeholders, no Okinawa names; guidance preserved; checks.rs untouched.
- Tests: rewritten `compare_content_depth_behavior_lock.rs` green (new gate cases + fixed `renders_...totals`); `verdict_*` green (verify no seed change needed).
- Docs: CLI.md updated; CLAUDE.md annotated; historical numbers untouched.
