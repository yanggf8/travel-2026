# Codex Verification Task: Stage 0 Research Shaping Feature

## Context & Goal

We are extending the Stage 0 "triangle research" system in this Japan travel planning codebase.

**Core philosophy of this change:**
- Stage 0 research is *dynamic and exploratory*.
- Previously we only had a simple `window_start` + `window_end` on `stage0_research_runs`.
- We needed a way to capture richer inputs discovered during research (hard constraints like "return must be ≤ 2026-06-27", "never use KKday", "Liko cannot travel on 6/28 due to 馬偕 commitment", mobility rules, hotel requirements, etc.).
- The user explicitly rejected using JSON blobs. Everything must be normalized relational tables.
- We deliberately named the new table **`stage0_research_shaping`** (not "constraints") because research is still fluid. `role = 'hard_constraint'` is only *one possible value*. Other roles include `soft_preference`, `search_directive`, `observed_signal`, `hypothesis`.

This feature was implemented to support real-world research like the current Okinawa (OKA) trip with strict personal constraints from Liko.

## What Was Changed

### New Table (source of truth in migration)
- `scripts/turso-migrate.ts` (and mirrored in `scripts/schema.sql`)
- New table: `stage0_research_shaping`

Schema (normalized, no JSON):
```sql
CREATE TABLE IF NOT EXISTS stage0_research_shaping (
  run_id TEXT NOT NULL,
  aspect TEXT NOT NULL,             -- 'date' | 'channel' | 'mobility' | 'lodging' | 'budget' | 'activity' | 'general'
  role TEXT NOT NULL,               -- 'hard_constraint' | 'soft_preference' | 'search_directive' | 'observed_signal' | 'hypothesis'
  kind TEXT NOT NULL,
  value_text TEXT,
  value_date TEXT,
  value_integer INTEGER,
  notes TEXT,
  created_at TEXT NOT NULL,
  PRIMARY KEY (run_id, aspect, role, kind, COALESCE(value_text,''), COALESCE(value_date,''), COALESCE(value_integer,0))
);
CREATE INDEX IF NOT EXISTS idx_s0_shaping_run ON stage0_research_shaping(run_id, aspect, role);
```

### Code Changes
- `src/services/stage0-service.ts`:
  - Added `ResearchShaping` interface
  - Extended `CreateRunInput` and `ResearchRun` with `shaping`
  - `createResearchRun` inserts shaping rows
  - `getResearchRun` fetches shaping
  - New helper: `getResearchShaping(runId)`
  - `deleteResearchRun` cleans up shaping rows

- `src/cli/commands/stage0.ts`:
  - Added `parseShapingEntry()` helper
  - `stage0-init` now accepts repeatable `--shaping ASPECT:ROLE:KIND:VALUE[:NOTES]`
  - `stage0-compare` prints shaping rules grouped by aspect (with [HARD] / [PREF] markers)
  - `stage0-export` now includes shaping data

- `docs/superpowers/specs/2026-05-22-stage0-triangle-research-design.md`:
  - Updated principles and data model section
  - Added full documentation for the new `stage0_research_shaping` table + usage examples

## Verification Instructions for Codex

You are to perform a thorough **verification and review** of this feature. Assume the human has already run `npm run db:migrate:turso`.

**Do the following verification steps yourself as much as possible** (using the tools available to you: reading files, running commands, inspecting code, etc.). Only ask the human for input when something *truly* requires a human action (e.g., they have not yet run the migration, or you need them to create a real run against their Turso instance).

### Verification Checklist

1. **Schema & Immutability**
   - Confirm the table definition is correct and follows project patterns (see how `stage0_research_destinations` and `stage0_research_durations` are handled).
   - Verify that shaping rows are only written at run creation time (immutability principle must be respected).
   - Check that `deleteResearchRun` properly cleans the new table.

2. **Service Layer Correctness** (`src/services/stage0-service.ts`)
   - Review the TypeScript types and query construction (use of `sqlText` / `sqlInt` escaping).
   - Confirm that existing calls to `createResearchRun` without `shaping` still work (backward compatibility).
   - Check that shaping data flows correctly through `getResearchRun` and `getResearchShaping`.

3. **CLI Usability** (`src/cli/commands/stage0.ts`)
   - Test the `--shaping` parser logic mentally / via code inspection for edge cases:
     - Date values
     - Integer values
     - Values containing colons (in notes)
     - Missing parts
   - Verify that `stage0-compare` output is readable and useful.
   - Confirm `stage0-export` includes shaping in the JSON.

4. **Documentation Quality**
   - Review the updated design spec section.
   - Is the rationale for naming it "shaping" (instead of constraints) clear?
   - Are the examples (especially the Liko Okinawa ones) accurate and helpful?

5. **Project Principle Alignment**
   - Does this follow the project's core rules?
     - No JSON blobs for state
     - Runs are immutable
     - Agent-first (easy for agents to pass rich data at creation time)
     - Normalized tables
   - Any violations or risks you see?

6. **Gaps & Future Work**
   - The Python aggregator (`scripts/stage0_research.py`) does **not** yet consume shaping data for filtering or ranking. Is this acceptable for now, or should it be addressed?
   - There is currently no way to add shaping after a run is created (only at `stage0-init`).
   - No propagation to the plan side on `stage0-adopt` yet.
   - List any other missing pieces or risks.

7. **Test Impact**
   - Look at `tests/integration/stage0-service.regression.test.ts` and other stage0 tests.
   - Do any tests need updating because they create research runs?
   - Should we add regression coverage for the new shaping path?

### Output Format Expected from You (Codex)

Please structure your final response like this:

**Verification Summary**
- Overall status (Solid / Minor issues / Needs rework)
- List of things that passed cleanly

**Findings & Issues** (numbered, with file + line references where possible)
- Severity: Critical / Important / Nice-to-have

**Recommendations**
- Immediate improvements
- Suggested follow-up work (e.g. adoption propagation, Python integration, tests)

**Questions for Human** (only things that genuinely require the human to act or answer)

Do **not** make code changes yourself unless the human explicitly asks. Your job is verification + clear reporting.

---

## Example Shaping Usage (for context)

Real-world example for the current Okinawa research:

```bash
npm run travel -- stage0-init \
  --origin TPE --start 2026-06-12 --end 2026-06-25 \
  --dest OKA:"Okinawa (OKA)" \
  --nights 3 \
  --shaping date:hard_constraint:return_no_later_than:2026-06-27 \
  --shaping date:hard_constraint:exclude_depart:2026-06-28:Liko 馬偕 commitment \
  --shaping channel:hard_constraint:exclude_source:kkday:prior bad experience \
  --shaping mobility:hard_constraint:no_car:true \
  --shaping lodging:soft_preference:location_requirement:central_naha_yui_rail_walkable
```

After creation, `stage0-compare --run <id>` should clearly display all these rules.

---

You have full access to the codebase. Be rigorous, precise, and direct. Focus on correctness, maintainability, and adherence to the project's established patterns.