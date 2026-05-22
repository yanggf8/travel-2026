# Stage 0 — Triangle Research: Design Spec

> **Date:** 2026-05-22
> **Status:** Approved design — ready for implementation planning
> **Scope:** A+ (narrow) — build the missing Stage 0 capability; do **not** rename or rewire P1–P5.
> **Related:** `docs/plans/2026-05-22-new-planning-flow.md` (the proposed flow this implements one stage of)

---

## 1. Purpose

The proposed planning flow (`docs/plans/2026-05-22-new-planning-flow.md`) replaces the linear
P1→P5 model with a research-first loop. Its **Stage 0 — Triangle Research** is the only stage
with no skill owner and no tooling: it explores departure date, destination, and flight price
*together*, before any of them is locked.

`/p3-flights` cannot own this — it declares `requires_processes: [process_1_date_anchor,
process_2_destination]`, so it needs dates and destination already chosen. Stage 0 runs
*before* that lock.

This spec defines the missing piece: a DB-backed research-session domain, an aggregator that
ranks flight candidates across destinations and durations, a CLI command to display the
ranking, and an orchestration skill that drives the loop.

**Out of scope:** renaming P1–P5 skills, flipping the proposal doc to "Adopted", rewiring
CLAUDE.md's Skill Decision Tree, a dashboard view for Stage 0. P1–P5 remain fully operational.

---

## 2. Principles

- **Turso is the source of truth.** Stage 0 research data lives in normalized DB tables, not
  JSON files. The contract is "Turso rows in, Turso rows out."
- **No JSON arrays as state.** Multi-valued inputs (destinations, durations) are normalized
  child rows, never JSON-blob columns.
- **The scraper's JSON output is an implementation detail, not a contract.** `scrape_date_range.py`
  prints human-readable progress to stdout and only emits machine JSON via `--output <path>`, so
  the aggregator captures results by passing a **transient temp file** and reading it back
  immediately. That temp file is not durable state, not the handoff format, and is deleted after
  parsing. Nothing reads a `scrapes/*.json` path back as state.
- **Research can exist before a plan exists.** Stage 0 tables are *not* `plan_id`-scoped. They
  join the existing global/unscoped tables (`destination_config`, `ota_sources`,
  `origin_config`, `global_config`). A research session may ultimately spawn zero, one, or
  several plans.
- **Objective ranking.** Candidates sort by flight price; personal trade-offs (leave-days) are
  shown as columns and used only as deterministic tie-breakers.
- **Runs are immutable.** A `run_id` is a fixed research input tuple — origin, date window,
  pax, exchange rate, destination set, and duration set. Those rows are written once at run
  creation and never edited afterward. "Re-run the aggregator" therefore means exactly one
  thing: retry the run's `failed`/`pending` scrape attempts. Changing *any* input (add/swap a
  destination, shift the window, change durations or pax) is a **new run** with a new
  `run_id`. This keeps retry semantics unambiguous and removes any stale-candidate hazard —
  there is no path by which a run's candidates can disagree with its inputs.

---

## 3. Data Model

Six new tables, all unscoped (keyed by `run_id`, not `plan_id`). Added idempotently to
`scripts/turso-migrate.ts`; mirrored into `scripts/schema.sql` (read-only DDL reference).

### 3.1 `stage0_research_runs` — one row per research session

| Column | Type | Notes |
|--------|------|-------|
| `run_id` | TEXT PK | timestamp-based id, e.g. `stage0-20260522-143000` |
| `origin_code` | TEXT NOT NULL | IATA, e.g. `TPE` |
| `pax` | INTEGER NOT NULL | passenger count |
| `window_start` | TEXT NOT NULL | earliest departure date considered (YYYY-MM-DD) |
| `window_end` | TEXT NOT NULL | latest departure date considered (YYYY-MM-DD) |
| `currency` | TEXT NOT NULL | currency of all `*_twd` price columns; `TWD` |
| `exchange_rate_usd_twd` | REAL NOT NULL | USD→TWD rate used to derive TWD totals (the `--exchange-rate` value passed to `scrape_date_range.py`) |
| `status` | TEXT NOT NULL | `started` \| `scraping` \| `ranked` \| `adopted` \| `failed` |
| `created_at` | TEXT NOT NULL | ISO timestamp |
| `updated_at` | TEXT NOT NULL | ISO timestamp |

> **Why persist the exchange rate:** `scrape_date_range.py` scrapes prices in USD and converts
> via `--exchange-rate` (default 32.0); its output `params` block does **not** record the rate
> used. Without storing it, the TWD totals in `stage0_candidates` are not reproducible or
> re-derivable. The aggregator passes one rate per run and records it here.

### 3.2 `stage0_research_destinations` — destination candidates for a run

| Column | Type | Notes |
|--------|------|-------|
| `run_id` | TEXT NOT NULL | FK → `stage0_research_runs.run_id` |
| `dest_code` | TEXT NOT NULL | airport IATA, e.g. `KIX`, `NRT` |
| `dest_label` | TEXT NOT NULL | human label, e.g. `Osaka/Kyoto (KIX)` |
| `sort_order` | INTEGER NOT NULL | display order |

PK: (`run_id`, `dest_code`).

### 3.3 `stage0_research_durations` — trip lengths for a run

| Column | Type | Notes |
|--------|------|-------|
| `run_id` | TEXT NOT NULL | FK → `stage0_research_runs.run_id` |
| `nights` | INTEGER NOT NULL | user-facing trip length |
| `duration_days` | INTEGER NOT NULL | `nights + 1` — the value passed to `scrape_date_range.py --duration` |

PK: (`run_id`, `nights`).

> The aggregator's scrape loop is the **cross product** of `stage0_research_destinations` ×
> `stage0_research_durations`. `scrape_date_range.py` handles the date-window sweep internally,
> so the aggregator never loops dates itself.

### 3.4 `stage0_candidates` — one ranked option per (destination, duration, departure date)

| Column | Type | Notes |
|--------|------|-------|
| `candidate_id` | TEXT PK | `{run_id}-{dest_code}-{depart_date}-{nights}n` |
| `run_id` | TEXT NOT NULL | FK → `stage0_research_runs.run_id` |
| `dest_code` | TEXT NOT NULL | airport IATA |
| `depart_date` | TEXT NOT NULL | YYYY-MM-DD |
| `return_date` | TEXT NOT NULL | YYYY-MM-DD |
| `nights` | INTEGER NOT NULL | trip length |
| `flight_total_twd` | INTEGER | **party total** (all pax), cheapest combined outbound+return; NULL if no flights found |
| `leave_days` | INTEGER | Taiwan leave days consumed, from `holiday-calculator.ts` |
| `rank` | INTEGER | 1-based rank within the run after sort; NULL until ranked |
| `verdict` | TEXT | short human note, e.g. `cheapest KIX option`, nullable |
| `adopted_plan_id` | TEXT | set when this candidate becomes a real plan; nullable; NULL = not adopted |

> **`flight_total_twd` is the party total, not per person.** `scrape_date_range.py` and
> `view:prices` already treat parsed `combined_cheapest_twd` as a pax-level total. Stage 0
> preserves that — never divide by pax.

### 3.5 `stage0_candidate_flights` — outbound/return leg detail per candidate

| Column | Type | Notes |
|--------|------|-------|
| `candidate_id` | TEXT NOT NULL | FK → `stage0_candidates.candidate_id` |
| `direction` | TEXT NOT NULL | `outbound` \| `return` |
| `airline` | TEXT | carrier name |
| `depart_time` | TEXT | HH:MM |
| `arrive_time` | TEXT | HH:MM |
| `duration` | TEXT | e.g. `2h 35m` |
| `nonstop` | INTEGER | 0/1 |
| `price_total_twd` | INTEGER | party total for this leg |

PK: (`candidate_id`, `direction`).

### 3.6 `stage0_scrape_attempts` — one row per (destination, duration) scrape

Makes partial failures visible and retryable — the aggregator scrapes the cross product of
destinations × durations, and any pair can fail independently.

| Column | Type | Notes |
|--------|------|-------|
| `run_id` | TEXT NOT NULL | FK → `stage0_research_runs.run_id` |
| `dest_code` | TEXT NOT NULL | airport IATA |
| `nights` | INTEGER NOT NULL | trip length of this attempt |
| `status` | TEXT NOT NULL | `pending` \| `ok` \| `failed` |
| `candidate_count` | INTEGER | candidates produced by this attempt; NULL until done |
| `error` | TEXT | failure message; NULL on success |
| `attempted_at` | TEXT | ISO timestamp of last attempt; NULL until first run |

PK: (`run_id`, `dest_code`, `nights`).

The aggregator inserts one `pending` row per pair before scraping, then updates it to `ok`
(with `candidate_count`) or `failed` (with `error`). A re-run retries only `failed`/`pending`
rows for the run.

---

## 4. Ranking

Within a run, after all scraping completes, `stage0_candidates` rows are sorted and assigned
`rank` (1-based). Sort order — deterministic, all ascending:

1. `flight_total_twd` ASC — objective primary metric (NULLs sort last)
2. `leave_days` ASC — tie-breaker only; never the primary signal
3. `depart_date` ASC — final deterministic tie-breaker

Leave-day cost is a personal trade-off, so it is **displayed as a column** and used only to
break price ties — never folded into the primary sort.

---

## 5. Components

### 5.1 Aggregator script — `scripts/stage0_research.py`

Python (consistent with the other scrapers it wraps).

**Inputs:** a `run_id` (the run + its destination/duration child rows must already exist in
Turso — created by the skill or a thin `stage0-init` step).

**Behaviour:**
1. Read the run, its destinations, and its durations from Turso.
2. Set run `status = scraping`.
3. For each (destination, duration) pair: insert a `pending` row into
   `stage0_scrape_attempts`, then invoke `scrape_date_range.py` with the run's origin, date
   window, `--duration duration_days`, `--pax`, and `--exchange-rate` (the run's
   `exchange_rate_usd_twd`). Results are captured by passing `--output <tempfile>` and reading
   that temp file back; the temp file is deleted after parsing — never a durable `scrapes/`
   path treated as state.
4. Parse each result's per-departure-date rows into `stage0_candidates` +
   `stage0_candidate_flights` rows. Compute `leave_days` per candidate via the leave
   calculator. Update the attempt row to `ok` with `candidate_count`.
5. After all pairs complete, rank (section 4) and write `rank` back.
6. Set run `status = ranked` (or `failed` on unrecoverable error).

**Failure handling:** if one (destination, duration) scrape fails, record it on its
`stage0_scrape_attempts` row (`status = failed`, `error` set), continue the others, and still
rank what succeeded. The run only goes `failed` if *no* combo produced candidates. A re-run of
the aggregator retries only the run's `failed`/`pending` attempt rows, leaving existing
candidates intact.

### 5.2 CLI command — `stage0-compare`

New command module `src/cli/commands/stage0.ts`, registered in `travel-update.ts`.
`requiresState: false` — Stage 0 is pre-plan, so it must not go through plan resolution.

```
npm run travel -- stage0-compare --run <run_id> [--json] [--limit N]
```

`--run` must be added to `OPTIONS_WITH_VALUES` in `src/cli/shared/args.ts` — otherwise the
shared parser does not treat it as a value-bearing option and the run id leaks into
`cleanArgs`.

Reads the run's candidates from Turso, prints a ranked cross-destination table:

```
Stage 0 Research — stage0-20260522-143000  (TPE, 2 pax, window 2026-06-18..2026-06-20)

 #  Dest  Depart      Return      Nights  Flight (party)  Leave  Verdict
 1  KIX   2026-06-18  2026-06-24  6       TWD 18,400      3      cheapest KIX option
 2  NRT   2026-06-19  2026-06-25  6       TWD 19,100      4
 3  KIX   2026-06-20  2026-06-27  7       TWD 21,800      3
 ...
```

This is the capability `view:prices` lacks — `view:prices` reads a single scrape file;
`stage0-compare` ranks across destinations and durations from the DB.

A companion `stage0-list` (list research runs) and `stage0-adopt <candidate_id> <plan_id>`
(record adoption) may be added — see section 6.

### 5.3 Orchestration skill — `/stage0-research`

New skill at `src/skills/stage0-research/SKILL.md`.

- **Frontmatter:** `requires_processes: []` — owns pre-lock research, depends on no process.
  `requires_skills: [travel-shared, scrape-ota]`.
- **Role:** orchestration. It does *not* replace `/p3-flights`; it runs *before* P1/P2 exist.
- **Workflow it drives:**
  1. Gather inputs from the user: origin, travel window, destination candidates (1–3),
     duration range.
  2. Create the run + destination/duration rows in Turso.
  3. Run `scripts/stage0_research.py` for the run.
  4. Show `stage0-compare` output.
  5. Loop: if the user wants to add/swap a destination, shift the window, or change
     durations/pax, that is a **new run** (runs are immutable — §2) — create a fresh
     `run_id` with the new inputs and scrape it. The previous run's candidates remain
     intact and comparable. (Re-running the aggregator on the *same* run only retries its
     `failed`/`pending` attempts.) Stop when the user locks a candidate.
  6. **Handoff:** on lock, hand the chosen candidate's date + destination to `/p1-dates`
     and `/p2-destination` (the normal flow takes over from there), and record the link via
     `stage0-adopt` (`adopted_plan_id`).

---

## 6. Adoption / handoff

When the user locks a candidate:
- `stage0-adopt <candidate_id> <plan_id>` sets `stage0_candidates.adopted_plan_id` and the
  run's `status = adopted`.
- The skill then invokes the existing `/p1-dates` (set the locked dates) and
  `/p2-destination` (set the locked destination) — unchanged.
- This is the only coupling between Stage 0 and the plan domain: a nullable
  `adopted_plan_id` pointer. Stage 0 tables remain otherwise independent.

---

## 7. Migration

- New tables added to `scripts/turso-migrate.ts` with `CREATE TABLE IF NOT EXISTS` (idempotent,
  matching the existing migration style).
- `scripts/schema.sql` updated to mirror the new DDL (it is a read-only reference extracted
  from the migration script).
- No data backfill — these tables start empty.
- No changes to any existing table.

---

## 8. Doc hygiene (carried in this scope)

These proposal-doc fixes were applied earlier and must remain consistent with this work:
- `CLAUDE.md` — the planning-flow doc is described as **proposed**, not "supersedes P1→P5".
- `docs/plans/2026-05-22-new-planning-flow.md` — Stage 0 Skill Mapping states `/p3-flights`
  **cannot** own pre-lock research; Stage 3 example uses `--fixed true`.

After implementation, the planning-flow doc's Skill Mapping row for Stage 0 should be updated
to name `/stage0-research` as the owner (replacing "no skill owner"). The doc stays
**Proposed** — building Stage 0 does not flip it to Adopted; that still requires resolving the
five open decisions and reconciling P1–P5.

---

## 9. Testing

Integration tests under `tests/integration/`, real DB, following the existing
`seed → dispatch → SELECT → assert → teardown` pattern (no mocks):

1. **Migration** — tables created idempotently; re-running migration is a no-op.
2. **Ranking** — given seeded `stage0_candidates` with known prices/leave-days, `rank` is
   assigned by the section-4 sort, including tie-breaker behaviour and NULL-price ordering.
3. **`stage0-compare`** — given a seeded run, the command prints rows in rank order and
   `--json` emits the candidates.
4. **Adopt** — `stage0-adopt` sets `adopted_plan_id` and run `status = adopted`.
5. **Scrape-attempt log** — given a run with seeded `stage0_scrape_attempts` rows, a mix of
   `ok` and `failed` still ranks the `ok` candidates; a run with all attempts `failed` goes
   `status = failed`.

The aggregator's subprocess scraping is **not** unit-tested (it depends on live OTA sites,
consistent with the rest of the scraper suite). Its *parsing* logic — scrape-result JSON →
candidate rows — should be covered by a fixture-driven test if practical.

---

## 10. Open items (do not block implementation)

These belong to the broader proposal, not to building Stage 0:
- The five open decisions at the bottom of `docs/plans/2026-05-22-new-planning-flow.md`.
- Whether to rename P1–P5 skills to S1–S4.
- A dashboard view for Stage 0 research runs.
