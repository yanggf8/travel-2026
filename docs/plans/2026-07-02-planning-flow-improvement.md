# Planning-flow improvement plan (evidence-based)

**Date:** 2026-07-02 · **Status:** Codex-reviewed (ACCEPT-WITH-CHANGES) + Claude-corroborated against
source. Codex's changes are folded in below and flagged `[Codex]`.
**Basis:** the ACTUAL operation history of the 3 completed trips (tokyo-2026, kyoto-2026,
okinawa-2026) in `operation_runs` / `plan_offers` / `shaping_*`, cross-checked against the documented
flow (`docs/plans/2026-05-22-new-planning-flow.md`) and CLAUDE.md's Skill Decision Tree. This critiques
the trip-PLANNING WORKFLOW, not the codebase.

## The documented flow (what we say we do)

```
Shaping Stage (triangle research: date × dest × flight) → shaping-adopt --create-plan
  → Stage 1 /stage1-itinerary-draft   (scaffold-itinerary; validate the choice)
  → Stage 2 /stage2-shop-transport    (/p3-flights, /p3p4-packages, /separate-bookings, select-offer)
  → Stage 3 /stage3-expand-itinerary  (booking-aware detail)
  → Stage 4 /stage4-publish-dashboard
```

## The evidence (what we actually did — DB, n=3 real trips)

| Signal | tokyo-2026 | kyoto-2026 | okinawa-2026 | Reading |
|---|---|---|---|---|
| total mutation ops | 70 | 48 | 327 | |
| **itinerary-detail ops** | 63 (90%) | 33 (69%) | 291 (89%) | **majority of ALL work** |
| **shopping ops** (select-offer/import/promote) | 0 | 0 | 0 | shopping loop never the mechanism |
| plan_offers rows | 4 | 2 | 0 | offers exist for 2 trips, recorded post-hoc |
| audited `select-offer` ops | 0 | 0 | 0 | documented select step never ran |
| `shaping-adopt` used | no | no | no | |
| shaping candidates / adopted (okinawa run) | — | — | 90 / **0** | 0 adopted |
| how the plan was born | (TS-era seed) | (TS-era seed) | `set-flight`/`set-hotel` typed directly, THEN `scaffold-itinerary` | flights+hotel pre-decided, hand-entered |
| okinawa edit timeline | — | — | Jun 10(136) 11(83) 13(3) 15(12) **17(91)** | trip Jun 12-16 → biggest burst AFTER; edits DURING |

Sources: `operation_runs` by `command_type`/day per plan; `plan_offers`, `plan_offer_selection`,
`shaping_candidates.adopted_plan_id` for `shaping-20260525-093508`.

## Findings

### F1 — Shaping→adopt never used; docs claim it was. (doc↔reality drift) — STRONG
Okinawa's shaping run: 90 candidates, **0 adopted**; **no `shaping-adopt` in `operation_runs`** — yet
CLAUDE.md says Okinawa was "adopted into"/"Originated from" the run. It was actually born by
`set-flight`/`set-hotel` typed directly. `[Codex]` This is not an n=3 inference — it's an **internal
contradiction** (docs vs `adopted_plan_id=0` + op history). Highest-confidence finding.

### F2 — Stage 2 (shop-transport) barely load-bearing — WEAKENED (heavily confounded)
0 shopping/select ops across all 3 trips; okinawa 0 offers. `[Codex + corroborated]` **Two confounds
make this weak, not medium:** (a) all 3 trips had **pre-decided flights** (survivorship — the case we
have data for); (b) **tool-maturity: `promote-offers` landed 2026-06-28 and `select-offer` 2026-06-09,
but okinawa was planned 2026-06-10 — so `promote-offers` DIDN'T EXIST when okinawa was built** (verified
via `git log`). Historical non-use is therefore NOT clean evidence of low value. F2 supports only:
**"Stage 2 needs a first-class known-booking path,"** NOT "Stage 2 is unnecessary."

### F3 — ~90% of real effort is itinerary detail; the flow under-serves it — STRONG (directionally)
Itinerary-detail ops dominate every trip (69–90%). `[Codex caveat]` op *count* over-weights itinerary
edits (they're granular), so treat it as "clearly under-served," not a precise 90%. The draft/expand
split is one continuous loop in practice (okinawa: 23 `delete-activity`, 28 `set-activity-title`).

### F4 — Lifecycle doesn't end at "publish"; plans mutate during & after the trip — MEDIUM-STRONG
Okinawa's biggest edit burst (91 ops) was **Jun 17, the day AFTER the trip** (retrospective
"（實際）"); edits also on Jun 13/15 (during). `[Codex]` Mostly one trip → model it LIGHTLY, extending
existing lifecycle, not as a new product surface.

### F5 — "Flights already known" is first-class in reality but has no first-class path — STRONG (observed)
All 3 trips had flights+hotel decided before the tool; the flow serves this only by IGNORING Shaping +
Stage 2. Supports a first-class constrained-trip path — NOT replacing Shaping.

### F6 `[Codex, corroborated]` — Instrumentation gap (the real root cause)
`operation_runs` **cannot distinguish** "stage intentionally skipped" / "stage unavailable" / "replaced
by known inputs" / "bypassed because docs were wrong." Every finding above is inferred from *absence*
of ops. Without recorded **skip/entry reasons**, future DB history will keep being ambiguous (and keep
"lying by omission" — e.g. no way to record "influenced by a shaping run" when `shaping-adopt` wasn't
used). Fixing this is prerequisite to trusting stronger conclusions later.

## Proposed improvements

**P0 `[Codex — highest-value first change]` — Add a first-class trip-intake router.**
A single entry that classifies the trip up front: `flexible research` | `fixed dates/destination` |
`known flights` | `known flights + hotel/package`. This unlocks P1, safely reframes P4/P6, and — via
F6 — **labels future history** so the ambiguity that forced this whole analysis doesn't recur. Do this
first, then fix the Okinawa doc drift (P2).

**P1 — Known-flights fast-path (fixes F1, F5).** Documented path: create plan → `set-flight`/`set-hotel`
→ itinerary, with Shaping explicitly OPTIONAL (only when dates/flights unknown). This is literally what
all 3 trips did. (One branch of the P0 router.)

**P2 — Correct the doc↔reality drift (fixes F1).** CLAUDE.md must stop asserting Okinawa was
`shaping-adopt`-ed. State the truth: flights/hotel pre-decided + hand-entered; the shaping run was
exploratory, not adopted. `[Codex]` Also add a way to record "influenced by shaping run <id>" without a
formal adopt, so provenance isn't lost.

**P3 — Unify Stage 1+3 wording but KEEP the draft→validate gate `[Codex — revised]`.**
Do NOT merge them into one undifferentiated loop. The May flow gives Stage 1 a real job: coarse
viability (pacing, lodging topology, go/no-go) BEFORE investing in detail. The 23 deletes are
ambiguous — they could mean "one loop" OR "the gate worked, it caught things to change." Keep **two
itinerary depths under one mental lane**: Stage 1 = coarse viability gate; Stage 3 = booking-aware
detail. Reduce duplicated wording/friction; preserve the gate.

**P4 — Stage 2 has MODES, not "optional" `[Codex — revised]`.** Reframe from a mandatory stage (and
from "make it optional") to explicit modes: `shop` (flexible/price-sensitive; compare direct vs
package), `ingest-known` (flights/hotel already chosen — record + validate + continue),
`defer` (explicitly decline; log skip reason). **Package/direct comparison is optional;
transport/accommodation validation is not.** Keeps the capability the flow was designed for; adds the
known-booking path the evidence demands.

**P5 — Extend the EXISTING lifecycle to pre/in/post-trip `[Codex — revised]`.**
Corroborated: the repo ALREADY has PAST/ACTIVE/UPCOMING logic (`status.rs:50-57`) + a
`pre-trip-checklist` skill. So EXTEND that, don't invent a heavy model: **pre-trip** (bookings,
readiness, deploy/share) → **in-trip** (quick corrections, actual timing, map/weather) → **post-trip**
(actualization: mark planned-vs-actual, retrospective cleanup). Okinawa's Jun 17 burst makes post-trip
actualization clearly legitimate. The earlier "polish a completed trip" work belongs here.

**P6 — Adaptive Shaping breadth, not "right-size down" `[Codex — revised]`.**
Not "shrink Shaping" (survivorship trap — the trips that would exercise it had pre-decided flights).
Instead: if flights known → don't run Shaping by default; if flexible → preserve the FULL triangle
sweep; if it runs broad → keep all candidates in DB but PRESENT a decision-ready top-N + tradeoffs
(90 candidates is fine to store, bad to show raw).

## Caveats on the evidence (honesty)
- **n=3, all pre-decided flights** → STRUCTURALLY guarantees Shaping + Stage 2 look unused. F2/F5/F6
  are "handle the pre-decided case + instrument it as first-class," NOT "Shaping/Stage 2 are useless."
- **Tool-maturity confound (verified):** the OTA offer pipeline (`promote-offers` 2026-06-28) postdates
  okinawa planning (2026-06-10), so Stage-2 non-use is partly "it didn't exist yet."
- tokyo/kyoto predate `scaffold`/Shaping (TS-era seed) → their 0-scaffold/0-adopt is partly historical.
  Okinawa is the only post-flow trip and the strongest F1/F5 evidence.
- F3/F4 (effort distribution, post-trip edits) are robust; F2 is the weakest and correctly softened.

## Recommendation
**Do P0 first** (trip-intake router — Codex's highest-value change; it labels future history and
unlocks the rest), then **P2** (fix the doc drift immediately). P1, P3, P5 are high-confidence,
directly evidenced, and cheap (docs/skills/routing). P4, P6, F6-instrumentation are design changes to
discuss. This is a docs/skills/routing + light-instrumentation change, NOT a code rewrite — the CLI
commands are fine; the STAGING MODEL around them should change to match how trips are really built.

## Codex review — folded in (2026-07-02)
Codex verdict: ACCEPT-WITH-CHANGES. Every point corroborated against source before adopting:
- F1 = internal contradiction (not n=3): kept STRONG.
- F2: **softened further** — confirmed the tool-maturity confound (`promote-offers` postdates okinawa)
  + survivorship; reframed to "needs a known-booking path," not "unnecessary."
- P4: "optional" → **modes** (shop/ingest-known/defer); validation stays mandatory.
- P6: "right-size down" → **adaptive breadth** (avoid the survivorship trap).
- P3: **keep the draft→validate gate** (don't merge into one loop); unify wording only.
- P5: **extend existing** PAST/ACTIVE/UPCOMING + `pre-trip-checklist` (corroborated they exist) rather
  than invent a lifecycle model.
- NEW **F6 instrumentation gap** + **P0 trip-intake router** (Codex's top pick), both added.
Corroborations I ran: `status.rs:50-57` (lifecycle exists ✓), `pre-trip-checklist/SKILL.md` (exists ✓),
`git log` on `promote_offers.rs`/`select_offer.rs` (2026-06-28 / 2026-06-09 vs okinawa 2026-06-10 ✓).
