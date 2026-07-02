# Shaping purchase-matrix — EXECUTABLE impl plan (multi-AI pipeline)

**Date:** 2026-07-02 · **Status:** READY to fan out. Codex-designed (shape A) + Claude-corroborated
against source (see "Corroboration" at bottom). Origin: the "purchase helper at the Shaping stage"
idea — the purchase decision belongs where the constraints already live (`shaping_rules`), NOT a
top-level trip-classification router (that mis-framing was removed 2026-07-02).
**Pipeline:** Claude writes → Codex reviews → Grok implements → Claude verifies line-by-line.

## What it is (Codex design, verdict shape A)

A **read-only** command `travel shaping-purchase-matrix --run <run_id> [--limit N] [--qualified-only]`
that, for one Shaping run, scores every purchase OPTION (the direct-flight candidate + each OTA package
offer) against that run's `shaping_rules` — hard rules as **GATES** (violate ⇒ DISQUALIFIED, shown with
the reason), soft rules as **NUDGES** (score). Plain-text matrix. **NO new schema** — pure reader/scorer
over existing tables. It complements, does not duplicate, `shaping-compare` (flight/date ranking) and
`shaping-baseline` (group-tour-vs-FIT methodology).

## Scope (v1) / deferred

**IN (v1):** read run header + `shaping_rules` + `shaping_candidates` + `shaping_tour_group_offers`;
normalize to one option shape; apply gates + soft scoring; sort; print plain text.
**DEFERRED (do NOT build):** any new "curated good options" table (YAGNI for a single-owner planner —
"good options" is already `channel/preferred_sources` rules + per-run offers); mutation/audit triad
(read-only command); persisted scores; fuzzy hotel-area geocoding; cross-run memory; dashboard.

## Global constraints (repo rules)
- Read-only: `db::connect_read`; NO writes, NO audit triad. Plain-text output only (no JSON).
- SQL via targeted queries / `travel-db` repos, bound params. No `sql_quote`.
- Real-Turso integration test with **panic-safe `Guard`**, skip cleanly if credless, `zz*` rows only.
- Per-task commit; `make check` + the named test green.

---

## T1 — `repo::shaping_purchase` reader (travel-db) [Claude — the read surface]

**Files:** new `rust/crates/travel-db/src/repo/shaping_purchase.rs`; `repo/mod.rs` (`pub mod`);
test coverage via T2's integration test (repo has no lib test target of its own here).

**Reads (all verified to exist):**
- run header: `SELECT pax, currency, origin_code FROM shaping_research_runs WHERE run_id=?1`.
- rules: `SELECT aspect, role, kind, value_text, value_date, value_integer FROM shaping_rules WHERE run_id=?1`.
- flight candidates: `SELECT candidate_id, depart_date, return_date, nights, flight_total_twd, leave_days,
  rank, verdict FROM shaping_candidates WHERE run_id=?1`.
- package offers: `SELECT offer_id, source_id, depart_date, return_date, nights, price_per_person_twd,
  title, hotel_name, hotel_star_rating, meals_included_count, departure_status, seats_available
  FROM shaping_tour_group_offers WHERE run_id=?1`.

**Produces** typed rows: `RunHeader{pax,currency,origin_code}`, `Vec<RuleRow>`, `Vec<CandidateRow>`,
`Vec<OfferRow>`. Pure data; NO scoring in the repo (scoring is command logic).
**Commit:** `feat(travel-db): shaping_purchase reader (run header + rules + candidates + offers)`.

## T2 — `travel shaping-purchase-matrix` command (scorer + render) [Claude — gate/nudge semantics]

**Files:** new `rust/crates/travel-cli/src/shaping_purchase.rs`; `main.rs` (`mod` + dispatch arm,
mirror an existing `shaping-*` arm); test `rust/crates/travel-cli/tests/shaping_purchase_matrix.rs`.

**Options (matrix ROWS):** one per flight candidate (`flight:<candidate_id>`, KIND=`direct`) + one per
package offer (`offer:<offer_id>`, KIND=`package`).

**Per-option total (for budget/scoring):**
- flight: `flight_total_twd` (already a party total).
- package: `price_per_person_twd * run.pax` (per-person → party). Show per-person too.

**GATES (hard_constraint OR intrinsic purchasability → OK | FAIL | CHECK | N/A; any FAIL ⇒
VERDICT=DISQUALIFIED). Verified kinds:**
- **AVAILABILITY (SYSTEM gate — not a shaping rule; Codex review #1, top fix).** A package you cannot
  buy is disqualified regardless of shaping rules. Verified: `departure_status` is populated on ALL
  offers with values `available | guaranteed | limited_qiang_gou | limited_last_2_flight_seats |
  booking_in_change | sold_out`.
    - `departure_status = 'sold_out'` ⇒ **FAIL** (reason "sold out").
    - `seats_available` present (non-null) AND `< run.pax` ⇒ **FAIL** (reason "seats N < pax M").
    - `departure_status` starts with `limited_` ⇒ OK but flag in REASONS ("limited seats").
    - `booking_in_change` or null/unknown ⇒ **CHECK** (never silent-pass).
    - flight candidates ⇒ N/A (no availability data captured for the flight leg).
- `date/hard_constraint/return_no_later_than` (value_date): option `return_date <= value_date` else FAIL.
- `date/hard_constraint/exclude_depart` (value_date): option `depart_date == value_date` ⇒ FAIL.
- `date/hard_constraint/depart_window` (value_text `YYYY-MM-DD..YYYY-MM-DD`): depart in window; if
  unparseable ⇒ CHECK (do NOT silently pass).
- `lodging/hard_constraint/exclude_hotel` (value_text): package `hotel_name` matches ⇒ FAIL; flight ⇒ N/A.
- `mobility/hard_constraint/*` (e.g. no-public-bus): FAIL only if the offer text/title explicitly
  requires public bus; otherwise CHECK (no structured mobility column — do NOT treat tour-coach/bus
  language as public bus; never silent-pass). Codex-confirmed CHECK is the honest default.
- (Budget as a HARD ceiling only if a `budget/hard_constraint/*` rule exists — none today; see NUDGE.)

**NUDGES (soft_preference → integer score delta). Verified kinds:**
- `budget/soft_preference/flight_max_twd` (value_integer) — a **FLIGHT PARTY-TOTAL cap**, NUDGE only
  (never disqualifies). Corroborated via the drill run: `flight_max_twd=18000` for `pax=2` vs
  candidates whose `flight_total_twd` were ~15,800–18,900 → the value is the **party total**, NOT
  per-person. So: flight option `flight_total_twd <= value_integer` ⇒ +2 else −2 (**no `/pax`** — party
  vs party). Package ⇒ CHECK/0 (a package total is not comparable to a flight-only cap) — do NOT
  penalize a package on it.
- `channel/soft_preference/preferred_sources` (value_text CSV): package `source_id` ∈ list ⇒ +2;
  flight ⇒ 0; package with source not in list ⇒ −1.
- `lodging/soft_preference/preferred_hotel_area` (value_text): package `hotel_name`/`title` contains
  it ⇒ +1; unknown ⇒ 0; known mismatch ⇒ −1; flight ⇒ 0.
- `search_directive`/`observed_signal`/`hypothesis` roles: context only, no score.

**Sort (Codex #6 — make it unambiguous):** primary key = `VERDICT != DISQUALIFIED` (**DISQUALIFIED
rows ALWAYS sort last, regardless of score**); then score desc; then TOTAL_TWD asc; then (flight)
leave_days asc; then depart asc. A `CHECK` gate leaves the option QUALIFIED unless another gate FAILs.

**COST_SCOPE (Codex #2 — direct-flight vs package totals are apples-to-oranges).** Every row carries a
`COST_SCOPE`: direct flight = `FLIGHT_ONLY` (excludes hotel); package = `PACKAGE_TOTAL`
(flight+hotel). Never compare a `PACKAGE_TOTAL` to `flight_max_twd`. The price sort is within
scope-awareness (don't present a FLIGHT_ONLY total as beating a PACKAGE_TOTAL on absolute price without
the label making the basis clear).

**Plain-text columns:**
```
#  OPTION            KIND     COST_SCOPE     SOURCE     DATES              N  PP_TWD  TOTAL_TWD  HOTEL          VERDICT  SCORE  GATES / REASONS
```
DISQUALIFIED rows are shown by default (Codex: you must see WHY a cheap option isn't viable), e.g.
`DISQUALIFIED  FAIL_AVAIL: sold out` or `DISQUALIFIED  FAIL_LODGING: excluded hotel 水之都那霸`.
`--qualified-only` hides them; `--limit N` caps.

**Fail loud:** unknown `run_id` (empty header), no `--run`. Empty candidates+offers → print "(no
options — import candidates/offers first)", exit 0.

**Test oracle (`tests/shaping_purchase_matrix.rs`, real-Turso, Guard):** seed a `zz`-run with
`preferred_sources`, a `flight_max_twd` (party total), an `exclude_hotel`, an `exclude_depart`; seed one
flight candidate (over the party cap) + several offers: one on the excluded hotel, one from a preferred
source (`departure_status='available'`), one `departure_status='sold_out'`, one on the excluded_depart
date. Assert:
- `sold_out` + excluded-hotel + excluded-depart options print `DISQUALIFIED` with the specific reason
  (`FAIL_AVAIL` / `FAIL_LODGING` / `FAIL_DATE`);
- the preferred-source `available` offer scores higher than a non-preferred one;
- the over-party-cap flight is nudged down (−2) but NOT disqualified;
- every row shows a `COST_SCOPE` (`FLIGHT_ONLY` for the candidate, `PACKAGE_TOTAL` for offers);
- DISQUALIFIED rows sort AFTER all qualified rows regardless of score;
- `--qualified-only` hides the DISQUALIFIED rows; output is plain text (no JSON).
Guard-teardown the zz run rows (rules/candidates/offers).
**Commit:** `feat(cli): shaping-purchase-matrix — score purchase options vs shaping constraints`.

## Sequence & delegation
- **T1 first** (reader) → **T2** (scorer/render + test).
- `[Claude]`: both — T1 is the read surface (DAL boundary), T2 is the gate/nudge semantics (getting
  hard-gate vs soft-nudge wrong gives bad advice). Grok may implement the plain-text render in T2 once
  the gate/nudge table above is fixed (it is) — but Claude owns the scoring logic + verification.

## Docs (after T2 green)
- Add to CLAUDE.md CLI Quick Reference + the Shaping section: `shaping-purchase-matrix --run <id>` as
  the "which purchase option fits my constraints" view, beside `shaping-compare`/`shaping-baseline`.
- Optionally add a doc-consistency assertion (the gate/nudge rule KINDS named in docs match the ones
  the command reads) — reuse the `validate.rs` planning-flow guard pattern only if it earns its keep.

## Corroboration (Claude, against source — before writing this plan)
- All mapped `shaping_rules` KINDS exist in real runs: `budget/flight_max_twd`,
  `channel/preferred_sources`, `date/{return_no_later_than,exclude_depart,depart_window}`,
  `lodging/{exclude_hotel,preferred_hotel_area}` ✓. **Fix vs Codex:** the budget rule is
  `flight_max_twd` (a FLIGHT cap, soft_preference), not a generic total ceiling — folded in above.
- Offer/candidate columns scored on all exist (`flight_total_twd`, `leave_days`,
  `price_per_person_twd`, `hotel_name`, `source_id`, `depart_date`) ✓.
- `shaping_research_runs.pax` exists (needed for per-person → party total) ✓.
- Sibling commands exist and differ: `shaping-compare` (flight/date rank), `shaping-baseline`
  (group_tour-vs-FIT methodology, `shaping.rs:908`) — the matrix's constraint-scoring is distinct, no
  duplication ✓.
- Read-only + no-new-schema keeps it inside the repo's DB-only / plain-text / no-JSON rules ✓.

## Codex spec-review — folded in (2026-07-02)
Codex reviewed this spec (SPEC-NEEDS-FIXES). Every finding corroborated against live Turso before
folding in:
- **#1 (top fix) availability gate** — spec read `departure_status`/`seats_available` but didn't gate
  on them. Corroborated: `departure_status` populated on all 130 offers (`sold_out`/`limited_*`/
  `available`/`guaranteed`/`booking_in_change`). Added a SYSTEM purchasability GATE (`sold_out` ⇒
  DISQUALIFIED; `seats_available < pax` ⇒ DISQUALIFIED; null ⇒ CHECK).
- **#2 cost-basis** — direct-flight (transport-only) vs package (flight+hotel) totals are
  apples-to-oranges. Added a `COST_SCOPE` column (`FLIGHT_ONLY`/`PACKAGE_TOTAL`); never compare a
  package total to `flight_max_twd`.
- **#3 `flight_max_twd` unit** — spec's `/pax` was WRONG. Corroborated via the drill run (18000 for
  pax=2 vs ~15.8–18.9k party totals) → it's a **party-total** cap; changed to `flight_total_twd <=
  value` (no `/pax`), NUDGE-only.
- **#6 sort** — made explicit: DISQUALIFIED always sorts last regardless of score; CHECK stays
  qualified unless another gate FAILs.
- **Confirmed correct (no change):** `mobility/no-public-bus` CHECK-default; stars/meals are
  nudge/display not gates; `flight_max_twd` never disqualifies.
