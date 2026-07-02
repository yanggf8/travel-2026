# Planning-flow improvement — EXECUTABLE impl plan (multi-AI pipeline)

**Date:** 2026-07-02 · **Status:** READY to fan out. Codex-advised scope + Claude-corroborated against
source (see "Corroboration" at bottom).
**Proposal it implements:** `docs/plans/2026-07-02-planning-flow-improvement.md` (findings F1-F6,
proposals P0-P6, already reviewed ACCEPT-WITH-CHANGES).
**Pipeline:** Claude writes plan → Codex reviews → Grok implements task-by-task → Claude verifies
line-by-line. Task tags: `[→ Grok]` self-contained w/ clear oracle; `[Claude]` schema/semantics/judgment.

## Scope (Codex-advised — first slice, NOT all P0-P6)

**IN:** F6 (minimal audited `flow-decision` instrumentation) + P0 (trip-intake router, DOCS routing
table) + P1 (known-flights fast-path, docs) + P2 (fix Okinawa doc drift) + P4 (Stage-2 modes, docs).
**DEFERRED (explicit — do NOT implement here):** P3 (Stage1+3 wording polish — safe but not
foundational), P5 (lifecycle STATE machine — extend existing PAST/ACTIVE/UPCOMING later), P6 (adaptive
Shaping breadth ALGORITHM). Rationale: docs-first + minimal instrumentation now; encode no staging
STATE machine, no auto-router that changes command behavior, and no Shaping-breadth algorithm until at
least one real FLEXIBLE-flights trip exercises them (all evidence so far is n=3 pre-decided-flight).

## Global constraints (repo rules — every task obeys)
- DB is sole source of truth; no JSON columns/blobs; no facts in code/docs that belong in the DB.
- **Reuse the audit triad** (`cascade::common`): the ONE new command writes `plan_events` +
  `plan_event_data` + `operation_runs` + bumps `plans.version`. **No `operation_runs` schema change**
  (Codex: avoid the schema fight). Corroborated: `insert_event`/`insert_kv_rows`/`record_operation`/
  `new_run_id` exist at `cascade/common.rs:132/182/291/329`; `catalog_audit.rs` is a copy-target.
- Plain-text CLI output only. Real-Turso integration tests with **panic-safe `Guard`** (`mod common;
  use common::Guard;`), skip cleanly if credless, `zz*`/`test-*` rows only.
- Per-task commit; each task ends green (`make check` + the named test). Pre-commit hook = Rust build +
  `validate data` (docs tasks still trigger it — keep the tree building).

---

## T1 — `flow-decision` instrumentation command (F6) [Claude — semantics + audit design]

**Why Claude:** defines a new audited event contract + reuses the triad; wrong keys pollute history.
**Files:** new `rust/crates/travel-cli/src/flow_decision.rs`; `main.rs` (`mod` + dispatch arm);
test `rust/crates/travel-cli/tests/flow_decision.rs`.

**Command:** `travel flow-decision <stage> <decision> [--mode <m>] [--reason <r>] [--source <s>]
[--plan-id <id>]`
- `stage` ∈ `shaping | itinerary | shop | publish` (CHECK-validated in the command).
- `decision` ∈ `enter | skip | mode`.
- `--mode` ∈ `shop | ingest-known | defer` (required when `decision=mode`, else rejected).
- `--reason` free text (e.g. `known_flights`, `flexible`, `already_booked`). One human string only.
  Trim; if empty after trim, omit the KV (don't write a blank `reason`). Same for `--source`.

**Implementation — mirror `set_activity.rs` (plan-scoped triad), NOT `catalog_audit.rs`.**
`catalog_audit.rs` is the GLOBAL pattern (`catalog_runs`, **NO `plans.version` bump**) — WRONG for a
plan-scoped recorder (Codex review 2026-07-02, corroborated: `catalog_audit.rs:7` says "NO version
bump"; `set_activity.rs:753-806` is the correct template). Exact call sequence Grok must follow:
```
parse + validate ALL args (fail loud before any write)
plan_id = plan_resolver::resolve_plan_id(args)            # honors --plan-id / $TRAVEL_PLAN_ID
conn = db::connect_write()
version_before = cascade::common::read_version(conn, plan_id)      # ALSO proves the plan exists (fail loud if not)
version_after  = version_before + 1
sort_order = cascade::common::next_timeline_sort_order(conn, plan_id)
now_iso = cascade::common::now_rfc3339();  now_db = cascade::common::now_db_datetime()
insert_event(conn, plan_id, "timeline", "", "", sort_order, "flow_decision", &now_iso, None, None)
insert_kv_rows(conn, plan_id, "timeline", "", "", sort_order, &kv)   # kv: stage,decision,[mode],[reason],[source]
record_operation(conn, plan_id, "flow-decision", &summary, version_before, version_after, &now_db)
```
- **`plan_events` keying (scope `timeline`):** `destination=""`, `process_id=""`, `sort_order` from
  `next_timeline_sort_order` (MAX+1 — no PK collision on repeated calls; no new concurrency handling
  expected — same race posture as every other timeline writer).
- **`operation_runs.command_summary` format (specified, not guessed):** `"{stage} {decision}"`, plus
  ` mode={mode}` / ` reason={reason}` when present. e.g. `"shop mode mode=ingest-known reason=known_flights"`.
- **Version bump IS correct** (Codex-recommended): storage is plan-scoped `plan_events` + `operation_runs`,
  so it must use the full triad; a non-bump here would invent a third audit pattern. (This is a durable
  plan-history entry, not global telemetry.)
- **Accepted-value constants live in `flow_decision.rs`** (`STAGES`, `DECISIONS`, `MODES`) as `pub`
  slices, so T4/T5 doc-consistency tests can assert the docs match the command (no drift).
- **Fail loud on:** unknown/absent plan_id (via `read_version`), `stage ∉ STAGES`, `decision ∉ DECISIONS`,
  `mode ∉ MODES`, `decision=mode` WITHOUT `--mode`, `--mode` supplied WHEN `decision != mode`, unknown
  flags, and any flag missing its value. `--reason` is NOT required for `skip` (routing docs show
  reasonless calls). No write occurs if any validation fails.

**This is the F6 prerequisite:** it lets P0/P4 record WHY a stage was entered/skipped/moded, so future
history is unambiguous (the whole reason this analysis was needed). It changes NO existing command's
behavior — it's a pure recorder the agent calls at routing points.

**Test oracle (`tests/flow_decision.rs`, real-Turso, Guard):**
- seed a `zztest` plan; run `flow-decision shop mode --mode ingest-known --reason known_flights`;
  assert exactly ONE `plan_events` row (event=`flow_decision`), the `plan_event_data` keys
  `stage=shop`/`decision=mode`/`mode=ingest-known`/`reason=known_flights`, ONE `operation_runs`
  row (command_type=`flow-decision`), and `plans.version` bumped by 1.
- assert `flow-decision shop mode` WITHOUT `--mode` exits non-zero, writes nothing.
- assert `flow-decision shaping enter --mode shop` (`--mode` when `decision!=mode`) exits non-zero.
- assert an invalid `stage` exits non-zero. Guard teardown (plan_events/plan_event_data/operation_runs).
- assert a happy `enter`/`skip` (no `--mode`) also writes the triad + bumps version (not just `mode`).
- constants-drift guard: a test that `STAGES`/`DECISIONS`/`MODES` are exactly
  `{shaping,itinerary,shop,publish}` / `{enter,skip,mode}` / `{shop,ingest-known,defer}` (T4/T5 assert
  the docs match these).
- **Commit:** `feat(cli): flow-decision — audited stage entry/skip/mode recorder (F6)`.

## T2 — Fix the Okinawa doc drift (P2) [→ Grok] + a validate check [Claude]

**Why split:** the CLAUDE.md text edit is Grok-able; the `validate data` guard touches the consistency
contract (Claude), mirroring the existing OTA-pointer check.
**Files:** `CLAUDE.md` (2 spots: line ~14 "Originated from…", line ~483 "was adopted into"); extend
`rust/crates/travel-cli/src/validate.rs` (the CLAUDE.md-content check block ~line 496-542); test
`tests/validate_flow_doc.rs`.

**Doc edit [→ Grok]:** replace the two false-provenance claims. Okinawa was NOT `shaping-adopt`-ed
(DB: shaping-20260525-093508 has 90 candidates, 0 adopted; no `shaping-adopt` in operation_runs; born
via `set-flight`/`set-hotel` typed directly). New wording, e.g.:
"okinawa-2026 — flights (CI120/CI121) + hotel were pre-decided and entered directly; the
`shaping-20260525-093508` run was exploratory and was NOT adopted (0 candidates adopted)."

**Validate check [Claude]:** extend the existing CLAUDE.md content check (proven pattern:
`validate.rs:496` reads CLAUDE.md, `:512` `content.contains`, `:531/:542` forbid stale text). Add: FAIL
if CLAUDE.md contains the false-provenance phrases `Originated from the ` + a shaping run id, OR
`adopted into` for okinawa. This locks the fix so the drift can't silently return.

**Test oracle (`tests/validate_flow_doc.rs`):** `validate data` passes on the corrected CLAUDE.md;
a unit/fixture assertion that the forbidden phrase triggers the new error. `validate data` still exits
0 overall (pre-commit gate).
- **Commit:** `fix(docs): correct Okinawa false shaping-adopt provenance + validate guard (P2)`.

## T3 — Known-flights fast-path in the Skill Decision Tree (P1) [→ Grok]

**Files:** `CLAUDE.md` (Skill Decision Tree section); `docs/plans/2026-05-22-new-planning-flow.md`
(note the fast-path as a first-class entry).
**Change:** add an explicit routed row: "I already know my flights/hotel → create plan → `set-dates` +
`set-flight`/`set-hotel` (+ `flow-decision shaping skip --reason known_flights`) → go straight to
itinerary; **Shaping is OPTIONAL, only when dates/flights are unknown.**" This is what all 3 real trips
did. Do NOT remove the Shaping path — add the parallel known-flights path beside it.
**Test oracle:** a doc-content assertion (extend `tests/validate_flow_doc.rs` or a `grep`-style check
in `validate data`): Skill Decision Tree contains a "known flights" path that does NOT require Shaping,
and names `flow-decision shaping skip`.
- **Commit:** `docs: known-flights fast-path in Skill Decision Tree (P1)`.

## T4 — Trip-intake router as a routing table (P0) [Claude — routing semantics]

**Why Claude:** defines the canonical classification the whole flow keys on; docs-only (NO code router
that changes behavior — Codex hedge).
**Files:** `CLAUDE.md` (Skill Decision Tree — add the intake table); `docs/plans/2026-05-22-new-planning-flow.md`.
**Change:** a **routing decision table** (docs, not code) classifying every new trip up front, each row
naming the path + the `flow-decision` call to record it:

| Intake class | Signal | Route | Record |
|---|---|---|---|
| `flexible research` | no dates/dest/flights fixed | Shaping Stage | `flow-decision shaping enter --reason flexible` |
| `fixed dates/destination` | dates+dest known, flights not | create plan → p1/p2 → shop | `flow-decision shop enter` |
| `known flights` | flights+hotel chosen | fast-path (T3) | `flow-decision shaping skip --reason known_flights` |
| `known package` | package booked | ingest-known + validate | `flow-decision shop mode --mode ingest-known` |

**Explicit hedge (write into the plan):** "No automatic code router; classification is agent-driven
per this table; no lifecycle state machine; no Shaping-breadth algorithm — until a real flexible-flights
trip exercises them."
**Test oracle:** doc-content assertion that the 4 intake classes + their `flow-decision` records are
present; (optional) a `flow_decision.rs` unit test that each of the 4 `--reason`/`--mode` combos the
table names is accepted by the command from T1 (ties the docs to the recorder).
- **Commit:** `docs: trip-intake router decision table + flow-decision wiring (P0)`.

## T5 — Stage 2 as explicit modes (P4) [→ Grok]

**Files:** `CLAUDE.md` + `docs/plans/2026-05-22-new-planning-flow.md` + `src/skills/stage2-shop-transport/SKILL.md`.
**Change:** reframe Stage 2 from mandatory-stage to **modes** — `shop` (flexible/price-sensitive;
compare direct vs package), `ingest-known` (flights/hotel already chosen → record + validate +
continue), `defer` (explicitly decline; log skip reason). **Package/direct comparison is OPTIONAL;
transport/accommodation VALIDATION is mandatory in every mode.** Each mode names its `flow-decision
shop mode --mode <m>` record.
**Test oracle:** doc-content assertion that Stage 2 docs name exactly the three modes
`shop`/`ingest-known`/`defer` and state validation is mandatory; the modes match T1's `--mode` CHECK
set (consistency between the command and the docs).
- **Commit:** `docs: Stage 2 as modes (shop/ingest-known/defer), validation mandatory (P4)`.

## Sequence & delegation
- **T1 first** (F6 recorder) — T4/T5 reference its `--mode`/`--reason` values; T3/T4 name its calls.
- Then T2 (doc-fix, independent), T3, T4, T5.
- `[→ Grok]`: T2 doc edit, T3, T5 (+ their doc-content test assertions).
- `[Claude]`: T1 (command + audit design), T2 validate-check, T4 (routing semantics + the T1↔docs tie).
- Grok implements T1 ONLY after Claude specifies exact event/KV keys above (done — they're fixed here).

## Test plan (summary — what "green" means)
1. `tests/flow_decision.rs` (real-Turso, Guard): triad written with exact keys; invalid inputs fail
   loud + write nothing; version bump. **The one behavioral test.**
2. `tests/validate_flow_doc.rs`: `validate data` passes on corrected docs; forbidden false-provenance
   phrase triggers the guard; Skill-Decision-Tree contains the known-flights path + the 4 intake
   classes + the 3 Stage-2 modes (docs-content invariants, not prose).
3. Consistency tie: the `--mode`/`stage` sets in `flow_decision.rs` == the modes/stages named in the
   docs (a test or a `validate` check asserting they don't drift).
4. Gate: `make check` + `cargo test -p travel-cli --test flow_decision --test validate_flow_doc` green;
   `./bin/travel validate data` 0 errors; `./bin/travel doctor` clean.

## Pipeline model assignment (Codex-advised; capability smoke-tested 2026-07-02)

**Capability verified** — the pipeline "plug" really handles the Claude 5 family: Claude Code 2.1.198
supports Sonnet 5 (`claude-sonnet-5`) + Fable 5 (`claude-fable-5`); the subagent `model` param accepts
`sonnet|opus|haiku|fable`; no local allowlist gates them (account/plan-gated only); the Grok plugin
accepts `--model`. Live smoke tests through the subagent path PASSED: `fable-5 ok: 42`,
`sonnet-5 ok: 72`.

**Assignment (do NOT change the global `~/.claude/settings.json` "model": "opus[1m]" — keep Opus as the
driver + final verifier):**

| Task | Kind | Model | Why |
|---|---|---|---|
| T1 flow-decision command | audited Rust + audit-triad semantics | **Opus 4.8** (`[Claude]`) | new behavior + DB event contract — strongest model, and it's the final verifier's own work |
| T2 validate guard | consistency-contract code | **Opus 4.8** (`[Claude]`) | touches `validate data` rules |
| T2 doc edit / T3 / T5 | mechanical doc/routing wording | **Fable 5** or **Grok** (`[→ Grok]`) | cheap prose/routing edits; Fable-5 fits per Codex ("wording edits, routing-table prose") |
| T4 router semantics | canonical routing table | **Opus 4.8** (`[Claude]`) | load-bearing classification the flow keys on |
| Codex review pass | adversarial review | **Codex (gpt-5.x)** | unchanged pipeline role |
| Grok implementation | code impl of specced tasks | **Grok** (`grok-composer-2.5-fast`) | unchanged pipeline role |
| **Final line-by-line verify** | adversarial verify of all delegated output | **Opus 4.8** — NEVER downgrade | Codex: "would not make Sonnet the final verifier for this plan" |

**Rule:** Sonnet 5 = cost-aware Claude planning/review where a strong-but-cheaper model suffices;
Fable 5 = mechanical doc/prose only (NOT audited Rust, DB semantics, or final verify); Opus stays the
verifier. Per-task overrides via the subagent `model` param / `--model`; no global setting change.

## Explicitly deferred (do NOT build here)
- **P3** Stage1+3 wording polish (keep the draft→validate gate; do not merge). Docs-only, later.
- **P5** lifecycle STATE — extend existing PAST/ACTIVE/UPCOMING (`status.rs:50-57`) + `pre-trip-checklist`
  when we model pre/in/post-trip; NO new state machine now.
- **P6** adaptive Shaping breadth ALGORITHM — until a real flexible-flights trip exists.

## Corroboration (Claude, against source — before writing this plan)
- Audit triad reusable for a new command: `cascade::common::{insert_event:132, insert_kv_rows:182,
  record_operation:291, new_run_id:329}`; live copy-targets `catalog_audit.rs`, `set_activity.rs`. ✓
  → T1 needs NO schema change.
- `validate data` already reads CLAUDE.md + does content assertions: `validate.rs:496` read, `:512`
  `contains`, `:531/:542` forbid stale text. ✓ → T2/T3/T4/T5 doc-oracles extend a proven check.
- Lifecycle already exists (`status.rs:50-57` PAST/ACTIVE/UPCOMING) + `pre-trip-checklist/SKILL.md`. ✓
  → P5 correctly DEFERRED to "extend", not "invent".
- Tool-maturity confound (why F2/Stage-2 is softened, why P4 is modes-not-removal): `promote-offers`
  landed 2026-06-28, `select-offer` 2026-06-09, vs okinawa planned 2026-06-10 (git log). ✓
