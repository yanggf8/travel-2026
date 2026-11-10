# Process status machine: per-process ladders, forward-only paths, atomic writers

Status: DESIGN v2 (2026-11-11).
- v1 → self-review: added Mode, unranked-from rule, scaffold as an explicit reset, kept the
  mark-booked eligibility rule.
- v1 → external review (Codex, plus an independent Claude reviewer; Muse, Kimi and agy had no quota)
  → v2:
  - Anchor ladder of its own, and reopen edges.
  - Missing-row policy.
  - select-offer as single-hop preflight.
  - Command-level transactions with preflight before any write.
  - Extracted publish gate.
  - set-accommodation audit event.
  - flow-decision defer realigned with CLAUDE.md.
  - Lifecycle exceptions.
  - Consumer audit.

## Problem (traced on live data, 2026-11-11)

One generic transition table (`cascade::common::allowed_transition_targets`) serves all six
processes, and every multi-hop mover plans its route with an unconstrained shortest-path BFS.

1. **Shortest paths regress or skip booking.**
   - `researching → confirmed` = researching → pending → confirmed
   - `populated → confirmed` = populated → pending → confirmed (never booked)
   - `researched → confirmed` = researched → researching → pending → confirmed
   - `selecting → confirmed` = selecting → skipped → pending → confirmed
   - `researching → populated` = researching → pending → populated

   Cause: `pending → confirmed` exists for P1/P2 but is shared by every process.
2. **P5 has no forward driver.** Only `scaffold-itinerary` writes P5 (`researching`). Every plan
   tops out at a hand-set `researched`. kyoto-nov-2026 sits at `researching` the day before departure.
3. **Writers bypass the machine.**
   - `scaffold-itinerary` does `INSERT OR REPLACE` with no validation.
   - `mark-booked` jumps `selected/populated → booked` with an inline upsert and no `status_changed`.
     kyoto-nov's event history therefore ends P3_4 at `selected` while the status is `booked`.
   - `promote/import-offers` set `pending → researched` (illegal) with no event.
4. **Self-loop events.** `select-offer` re-runs logged 12 `selected→selected` /
   `populated→populated` events on kyoto-nov.
5. **Four private BFS copies** (set-process-status, set-accommodation, clear-accommodation,
   flow-decision).
   - clear-accommodation swallows a no-path error after deleting the booking rows.
   - flow-decision swallows advancement errors (`let _ =`).
6. **Validation after destructive writes, and no atomicity.**
   - select-offer rewrites flight_legs and hotels before it checks the P3/P4 transitions.
   - set-accommodation writes bookings before it checks P4.
   - Every multi-hop mover writes hop by hop with no transaction. A failure between `booking` and
     `booked` strands the row, and mark-booked (eligible only from selected/populated) cannot
     resume it.
7. **`confirmed` is a dead end.** Its only edge is `skipped`, so a confirmed P4 cannot be cleared
   and a confirmed P1 cannot be reopened in one command.

## Design

### 1. One module: `cascade/status_machine.rs`

```rust
pub(crate) enum Ladder { Anchor, Booking, Itinerary }
pub(crate) fn ladder(process_id: &str) -> Result<Ladder, String>       // unknown id → Err
pub(crate) fn targets(l: Ladder, from: &str) -> &'static [&'static str]
pub(crate) fn rank(status: &str) -> Option<u8>                         // None: skipped, cancelled
pub(crate) enum Mode { Forward, Any }
pub(crate) fn plan_path(l: Ladder, from: &str, to: &str, mode: Mode) -> Result<Vec<Hop>, String>
pub(crate) async fn apply_path(conn, plan_id, dest, process_id, from: Option<&str>, hops: &[Hop],
                               to: &str, now_iso, now_db) -> Result<(), String>
```

- `plan_path` is pure and has no I/O. Every caller computes **all** of its paths first (the
  preflight). It writes only when every path is Ok.
- `apply_path` writes each hop through `repo::process_statuses::upsert` and
  `emit_status_changed`. It **does not** call `record_operation` and does not open a transaction;
  the command owns both (§4).
- `from == to` gives an empty path: no write and no event.
- Ladder by process: P1, P2 → Anchor; P3_4, P3, P4 → Booking; P5 → Itinerary.
- Rank: pending 0 < researching 1 < researched 2 < selecting 3 < selected 4 < populated 5 <
  booking 6 < booked 7 < confirmed 8.
- Error strings keep the current wording (`unknown current status: …`,
  `no legal status path from … to …`), because `tests/set_process_status.rs` asserts them.

### 2. Ladders

| Ladder | Edges |
|---|---|
| **Anchor** (P1/P2) | pending→{researching, confirmed, skipped}; researching→{researched, pending, skipped}; researched→{confirmed, researching, skipped}; confirmed→{pending, skipped}; skipped→{pending}. No selecting or booking states. |
| **Booking** (P3_4/P3/P4) | today's table **minus `pending→confirmed`**, **plus `confirmed→booked`** (reopen). Confirmed is reachable only through booked, and leaving confirmed goes back through booked (e.g. clear-accommodation from confirmed = confirmed→booked→cancelled→selecting). |
| **Itinerary** (P5) | pending→{researching, skipped}; researching→{researched, pending, skipped}; researched→{confirmed, researching, skipped}; confirmed→{researched, skipped}; skipped→{pending}. `confirmed` = the itinerary is final, ready to travel. |

- `validate_transition(ladder, from, to)` checks a single hop against the process's ladder.
- `set-process-status` rejects a target outside the process's ladder. Example: `p5 booked` →
  `status booked is not on the itinerary ladder`.
- `cancelled` is added to the status parser, so a stranded `cancelled` row is a valid start state.
- **Legacy check:** live P5 values today are only `researching` and `researched`. No P5 row holds an
  off-ladder status, so no backfill is needed. `validate data` gains an Error for any process row
  whose status is not on its ladder, so drift fails loud.

### 3. Path planning

- **Forward target** (`rank(to) > rank(from)`, both ranked): BFS over rank-increasing edges only.
  - No skipped, cancelled or pending intermediate.
  - No forward path → Err. It never falls back to the unconstrained search.
- **Backward or special target** (`to` ranks lower, or is pending/skipped/cancelled):
  - BFS over all ladder edges.
  - An intermediate may not be `pending` or `skipped` unless it is the target.
  - Allowed only in `Mode::Any`; in `Mode::Forward` this is an Err.
- **Unranked `from`** (skipped/cancelled), allowed in both modes:
  - A fixed first hop: skipped → pending; cancelled → selecting when `rank(to) ≥ selecting`,
    otherwise cancelled → pending.
  - Then the forward rule applies.
  - Example: domestic `skipped → booked` = skipped→pending→populated→booking→booked.
- **Missing row** (`from = None`):
  - set-process-status: Err `no process_statuses row …`, as today.
  - The inserting writers (select-offer, set-accommodation, scaffold, promote/import-offers,
    flow-decision): insert the target directly with one `status_changed` whose from_state is NULL,
    as today. flow-decision keeps treating None as pending for planning.
  - `tests/select_offer_status_upsert.rs` locks this.
- **Lifecycle exceptions** are not transitions and are left alone:
  - `repo::plan_lifecycle` seeds P1/P2 `confirmed` and P5 `pending` on create-plan or adopt.
  - `copy-plan` copies rows.
  - `db seed-test-plan` reseeds.

### 4. Writers: preflight, then one transaction per command

Every listed command, in this order:
1. Reads its statuses.
2. Runs `plan_path` for every process it will move, plus the P5 publish gate (§5) where it applies.
3. Only if all of that is Ok: `BEGIN IMMEDIATE`, then domain writes, `apply_path` calls, domain
   events, and `record_operation` with the version bump, then `COMMIT`. ROLLBACK on any error.
   - Same pattern as set-fit-offer and copy-plan: one hrana stream; print only after COMMIT.
   - If hrana rejects `BEGIN IMMEDIATE`, use `BEGIN` and note the residual concurrent-writer race.
     That race is pre-existing (sort_order and version are MAX+1 reads) and is narrowed, not
     closed, here.

| Writer | After |
|---|---|
| set-process-status | `plan_path(target, Any)`; P5 → confirmed runs the gate first. |
| set-accommodation | `plan_path("booked", Forward)` before the booking writes. From confirmed: Err (already past booked). Always emits a booking-change domain event (`accommodation_booking_updated`, KV: hotel, price, date), even with no status hop. This closes the audit gap when a booked stay's price is corrected. |
| clear-accommodation | `plan_path("selecting", Any)` before the deletes. No path → Err and nothing deleted. |
| flow-decision `shop mode --mode defer` (domestic) | P3_4 and P3 → `skipped` (Any). **P4 is no longer skipped**, which realigns with CLAUDE.md ("P4 accommodation is NOT skipped — a domestic trip still books a stay"). `tests/domestic_flow_decision.rs:141` locks the old, contradicting behaviour and is updated. Errors propagate; no `let _`. |
| mark-booked | Eligible from selected, populated **or booking** (resumes a stranded booking). Then `plan_path("booked", Forward)` per eligible process. Keeps the booking_confirmed event (it allocates its sort_order after the hops), the dirty-flag clear, the focus and next-action writes, and the `selected → booking → booked` output line. Drops its `event_log_dest_processes` write: no active reader; the retired TS loader is archived. Update `tests/mark_booked.rs:44,175-185`. |
| select-offer | **Single-hop semantics, no multi-hop.** Preflight before step 1: P3_4 → selected must be equal or one legal edge (researched/selecting/populated/booking → selected; booked refused). P3/P4 → populated must be equal or one legal edge (pending/selected → populated; booking/booked refused). Any failure → Err, nothing written. "Equal" skips only the status write and event: a different offer still writes the selection, `offer_selected`, the populate rows and the audit. Event order stays as `tests/select_offer_cascade.rs:279-283` locks it. |
| promote-offers / import-offers | P3_4 → researched only when the current status ranks below researched or is missing (Forward; pending walks pending→researching→researched with events). Never regresses. |
| scaffold-itinerary | `plan_path("researching", Any)` **before** the skeleton is replaced. A re-scaffold is a logged reset (e.g. confirmed → researched → researching), and the hops are printed. `tests/scaffold_itinerary_behavior_lock.rs:322-375` is updated for the added status_changed rows. |

Deleted: `repo::itinerary::upsert_process_status_replace`, mark-booked's inline upsert, and the four
private BFS copies.

### 5. P5 confirm gate

- Extract from `validate::run_publish` a
  `pub(crate) async fn collect_publish_issues(conn, plan_id, dest) -> Result<Vec<PublishIssue>, String>`.
- `run_publish` keeps the printing and `exit(1)`.
- `set-process-status p5 confirmed` calls it in the preflight. Any `PublishSeverity::Block` refuses
  the confirm, nothing is written, and the blockers are listed. Warn and Info do not block.
- Past plans keep whatever `collect_publish_issues` returns for them (the existing `is_past`
  handling).

### 6. Consumers of "booked"

- A P4 can now legally be `confirmed`. Any process-status consumer that tests `== "booked"` must
  accept `confirmed` too.
- Already done in the booked-FIT change: `render/summary.rs` (FIT section, domestic stay block) and
  `validate.rs` fit_booking.
- The implementer greps the CLI and the worker for the remaining process-status comparisons. The
  booking-ledger statuses in `bookings_current` are a different domain and stay as they are.

### 7. History is not rewritten

- Past self-loop and missing mark-booked events stay as they are. Deleting rows needs Yang's say-so.
- kyoto-nov-2026's P5 moves once through the fixed command: two hops plus one audit row, after the
  gate passes.

## Tests

- **Pure, status_machine, every ladder × every (from, to, mode):**
  - Every hop is a ladder edge.
  - A forward path is strictly rank-increasing, with no skipped, cancelled or pending intermediate.
  - Booking `X → confirmed` contains `booked`.
  - Anchor `researching → confirmed` = [researching→researched, researched→confirmed], with no
    booking states.
  - Itinerary `researching → confirmed` takes two hops.
  - Anchor and Booking confirmed → pending/selecting are reachable in Any.
  - Forward never contains a backward hop.
  - `skipped → booked` in Forward = skipped→pending→populated→booking→booked.
  - `from == to` is empty.
  - Unknown process id or off-ladder target → Err.
- The four old BFS unit tests move to the shared planner.
- **Integration** (real Turso, zz_test_ fixtures, Guard armed early, run in the background):
  - mark-booked emits booking and booked events and resumes from `booking`.
  - scaffold on a confirmed P5 logs confirmed → researched → researching.
  - select-offer: a booked P4 is refused with **no** writes (selection, legs, hotel all unchanged);
    a re-run emits no from==to event but does record a different-offer selection.
  - set-process-status p5 confirmed: refused on a publish blocker; two hops when clean.
  - clear-accommodation: confirmed → selecting works; a no-path case writes nothing.
  - set-accommodation on a booked P4 writes the booking-change event.
  - Domestic defer leaves P4 untouched.
  - A multi-hop failure injected mid-command leaves nothing written (a test seam like copy-plan's
    `TRAVEL_COPY_PLAN_FAIL_AT`).
- Updated locks: `select_offer_status_upsert.rs` (unchanged expectations, re-run),
  `scaffold_itinerary_behavior_lock.rs`, `mark_booked.rs`, `domestic_flow_decision.rs`,
  `set_process_status.rs`.

## Out of scope

- The booked-FIT marker (separate change).
- Rewriting past events.
- A full fix of the cross-session sort_order and version race (narrowed only).
