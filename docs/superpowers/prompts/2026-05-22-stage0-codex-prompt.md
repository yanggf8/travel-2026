# Codex Prompt — Implement Stage 0 Triangle Research (all 10 tasks)

> Paste this whole file as the Codex prompt. It is self-contained: it tells Codex
> what to build, which files to use as the authoritative source, and the exact
> guardrails / verification gates between tasks.

---

## Role & Scope

You are implementing **Stage 0 — Triangle Research**, a new pre-plan research
capability for the `travel-2026` repository. The full implementation plan,
including the code to write for every task, is at:

```
docs/superpowers/plans/2026-05-22-stage0-triangle-research.md
```

That plan is **authoritative**. Read it in full before writing any code.
It contains 10 tasks with exact file paths, complete code blocks, exact shell
commands, and expected outputs for every step. Do not paraphrase or "improve"
its code — copy it verbatim unless you find a real bug, in which case stop and
ask before deviating.

The related spec (background, why these tables exist, ranking rules,
immutability principle) is at:

```
docs/superpowers/specs/2026-05-22-stage0-triangle-research-design.md
```

Skim it once for context; the plan is what you execute.

---

## Working environment (already set up)

- **Branch:** `feat/stage0-triangle-research` (already created, already
  checked out). Do not switch branches. Do not push.
- **Baseline:** `master` at commit `fe67f8a`. All existing tests pass
  (`24/24`). Typecheck passes. Working tree is clean.
- **Database:** the Turso database in `.env` (`TURSO_URL` / `TURSO_TOKEN`) is
  the project's shared production DB. The migration in Task 1 uses
  `CREATE TABLE IF NOT EXISTS`, so it is additive and idempotent. Smoke tests
  clean up their own rows. **Do not write any migration that drops, alters,
  or renames existing tables.**

---

## Hard rules

1. **Follow the plan task-by-task, in order, 1 → 10.**
2. **TDD where the plan says TDD.** For Tasks 2 and 3, write the failing test
   first, run it, confirm it fails for the stated reason, then implement, then
   confirm it passes. Don't skip the "verify it fails" step.
3. **Commit at the end of every task** with the exact `git commit -m` message
   the plan specifies. Do not amend, do not squash, do not rebase. One task =
   one (or more, if the plan says so) commit(s).
4. **Stop and ask** if:
   - A plan code block doesn't compile, but you've copied it verbatim.
   - A verification step's output disagrees with the plan's "Expected" text.
   - You find a real bug in the plan code.
   - The migration step fails for any reason.
   - Any test in the existing suite (the 24 baseline tests) starts failing.
5. **Do not invent commands or flags.** If a step says `./bin/travel db exec "..."`,
   use exactly that. The repo's CLI is sensitive — `--plan-id` and `--plan` are
   different, `cleanArgs` depends on `OPTIONS_WITH_VALUES`.
6. **Keep SQL escaping in TypeScript.** All SQL writes go through
   `src/state/sql-helpers.ts` (`sqlText`, `sqlInt`, `sqlReal`). The Python
   aggregator (Task 6) performs **zero** Turso I/O — it uses `stage0-export`
   (read) and `stage0-import` (write). Never build an SQL string from a CLI
   arg in Python.
7. **Honor immutability.** A `run_id` fixes origin, window, pax, exchange
   rate, destinations, and durations. The implementation must never edit those
   rows after creation; only `status`, `updated_at`, `rank`, and
   `adopted_plan_id` mutate post-creation.
8. **Smoke-test cleanups are required.** Every smoke test in the plan (Task 5
   Step 5, Task 7 Step 6) ends with a `db:exec` DELETE. Run it — do not leave
   smoke-test rows in the DB.

---

## What gets built (high-level map)

| Task | What | Files touched |
|------|------|---------------|
| 1 | Migration: 6 unscoped Stage 0 tables + index | `scripts/turso-migrate.ts`, `scripts/schema.sql` |
| 2 | `stage0-service` — types, `createResearchRun`, `getResearchRun`, `getScrapeAttempts`, `deleteResearchRun` (+ 2 tests) | `src/services/stage0-service.ts`, `tests/integration/stage0-service.regression.test.ts` |
| 3 | `stage0-service` — candidates, ranking, scrape-attempts, adopt, `deleteCandidatesForPair` (+ 4 tests) | same two files |
| 4 | Register `--run`, `--file`, `--origin`, `--rate` in `OPTIONS_WITH_VALUES` | `src/cli/shared/args.ts` |
| 5 | CLI: `stage0-init`, `stage0-compare`, `stage0-adopt` + static help | `src/cli/commands/stage0.ts` (new), `src/cli/travel-update.ts`, `src/cli/commands/help.ts` |
| 6 | Python aggregator (no Turso I/O; calls `stage0-export`/`stage0-import`) | `scripts/stage0_research.py` |
| 7 | CLI: `stage0-export`, `stage0-import` (idempotent per pair) | `src/cli/commands/stage0.ts` |
| 8 | `/stage0-research` orchestration skill | `src/skills/stage0-research/SKILL.md` |
| 9 | Docs: CLAUDE.md skills table + Turso DB section; planning-flow Skill Mapping row | `CLAUDE.md`, `docs/plans/2026-05-22-new-planning-flow.md` |
| 10 | Final verification: `make test`, `make check`, `./bin/travel doctor`, help check | — |

**Final test count after Task 3:** the new `stage0-service.regression.test.ts`
must have **6 passing tests** (2 from Task 2 + 4 from Task 3). Total project
test count goes from 24 → 30.

---

## Key codebase conventions (already in the plan, repeated here for safety)

- `TursoPipelineClient` from `scripts/turso-pipeline.ts`:
  - `.execute(sql)` — one statement
  - `.executeBatch(sqls)` — read batch; results at `response.results[i]`
  - `.executeMany(sqls)` — write chunks
- **No parameter binding** — always build values with `sqlText` / `sqlInt` /
  `sqlReal` from `src/state/sql-helpers.ts`. Read rows back with
  `rowsToObjects` / `rowsToObjectsAt`.
- **CLI commands** are `CommandHandler` objects (`src/cli/shared/types.ts`),
  registered via `registerCommand()`, imported in `src/cli/travel-update.ts`.
  For pre-plan commands set `requiresState: false` so plan resolution is
  skipped.
- **Tests** are integration-only, real DB, no mocks. Pattern:
  seed → act → assert → teardown. Every test using a run id must register it
  with `uniqueRunId()` so `afterAll` cleans it up.
- **`run_id` format:** `stage0-YYYYMMDD-HHMMSS`. Tests use
  `stage0-test-<ts>-<rand>` to avoid colliding with real runs.

---

## Per-task gate (between every two tasks, do this)

After committing a task, before starting the next one:

1. `git status` — must be clean (no untracked, no uncommitted).
2. `git log --oneline -5` — confirm your commit landed with the expected
   message.
3. `make check` — must print `✅ Typecheck passed`.
4. If the task wrote or touched tests, `make test` must still be all green.

If any of those fail, **stop and report** — do not start the next task.

---

## Execution

Begin with **Task 1**. Read its full body in the plan, run each step in
order, verify each "Expected" output, then commit. Move on to Task 2. Continue
until Task 10's verification all-green. Do not skip the verification gates
inside individual tasks (especially smoke tests + their cleanups in Tasks 5
and 7).

When Task 10 is green, report:
- The final commit log for this branch (`git log master..HEAD --oneline`).
- `make test` summary (test count + pass/fail).
- Anything you flagged as "stop and ask" but worked around with an explicit
  note (ideally nothing — prefer to stop and ask).

That's the entire job. The plan tells you what to type at each step; this
prompt tells you the rules. Don't deviate from either.
