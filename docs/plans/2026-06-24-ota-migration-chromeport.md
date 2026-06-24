# OTA Migration to chromeport — full source verification + Python decommission

**Status:** PLAN (awaiting Codex review → corroborate → agent appraisal)
**Date:** 2026-06-24
**Scope:** Migrate ALL OTA (online travel agent) sources off the dead archived Python
scrapers onto the live `chromeport` CDP pipeline, reaching a *verified* state per source
so each archived parser can be deleted.

---

## 1. Why this plan exists (ground truth, not assumption)

The earlier mental model — "settour works, replicate it to the other 9" — is **wrong**.
Live DB inspection (2026-06-24) shows:

- **No OTA source is live-verified.** All 10 active sources in `parser_rules` have
  `has_custom_parser=0` AND an identical *seed* `fetched_at` (`2026-06-10`). The `captures`
  table holds only stale seed captures (`2026-06-06` / `2026-06-10`) for OTA sources — no
  fresh live capture exists for any of them.
- **settour's custom Rust parser is dark.** `parse_settour` exists in code
  (`chromeport/src/main.rs:2581`) but its `parser_rules` row has `has_custom_parser=0`, so
  `parse capture` runs the *generic* regex path and never calls it. The flag is off in
  `default_parser_rules()` (`main.rs:1577`).
- The archived Python scrapers (`archive/broken-python-scrapers/`) are **dead** — URL
  templates 404. They must never run. `chromeport` replaces them.

So the real task is: **get from ~0 verified → N verified**, per source, each via a real
live Chrome capture + `verify` + `parse`, then delete the matching archived Python file.

## 2. Two CDP toolsets — division of labor

Both `chromeport` (travel repo, Rust) and `gwebcdb/bridge` (shared, Python) attach to the
**same** Chrome on `:9222`. They are complementary, not competing.

**Decision: chromeport does all OTA scraping. gwebcdb is NOT in the OTA path.**

Rationale: `chromeport fetch interact` already implements the same primitives gwebcdb
offers (`fill:SEL=VAL` with React-aware event dispatch, `click:SEL`, `wait:MS`,
`waitfor:SEL`) AND already owns the downstream pipeline gwebcdb entirely lacks —
capture→verify→parse→`offers` table with regex `parser_rules`. gwebcdb's bridge has **no
structured extraction** (only the E*TRADE-specific `positions.py`); routing OTA work
through it would add a Python subprocess boundary for zero gain.

**The one place gwebcdb could win** (and only if it arises): an OTA that requires an
authenticated session — gwebcdb's approval-gated `login_assist` keeps credentials out of
the CLI. That's a human-in-the-loop prerequisite step *before* chromeport captures; not
part of the automated path. (tigerair/trip are the only candidates; confirm logged-out
results pages render price first — they likely do.)

| Phase | Tool |
|---|---|
| Drive search form (dates/dest/pax) | chromeport `fetch interact --step` |
| Capture results page (`body.innerText`) | chromeport (auto, end of `fetch interact`) |
| Store capture | chromeport → Turso `captures` |
| Regex diagnostics (read-only) | chromeport `verify` |
| Parse + import to `offers` | chromeport `parse capture` |
| Login (only if an OTA forces it) | human, or gwebcdb `login_assist` (approval-gated) |

## 3. The per-source migration unit (parallelizable checklist)

Prereqs: Chrome on `:9222` (automation profile, not daily Chrome); `chromeport browser
doctor` OK; Turso tokens exported (`TRAVEL_TURSO_{URL,READ_TOKEN,WRITE_TOKEN}`).

1. **Rule row exists** — `db query "SELECT … FROM parser_rules WHERE source_id='<id>'"`;
   if missing, `parser rules seed-defaults`.
2. **Find the live URL** — navigate the OTA's own search UI in Chrome to the product/results
   page for the target trip (Naha, 2026-06-12) and confirm real offer data renders.
3. **Capture** — GET-able page: `fetch url "<url>" --source <id>`; form-driven:
   `fetch interact "<url>" --source <id> --step 'click:…' --step 'fill:…=…' --step
   'wait:1500' --step 'waitfor:.results'`. Record the `capture_id`.
   - **Set pax=2 in the form** (project default) so `pax_divisor=2` math is correct (risk 6c).
4. **Verify (read-only)** — `verify <source_id> <capture_id>`; read per-field OK/MISSING and
   the `snippet`.
5. **Tune `parser_rules` on MISSING** — adjust the offending regex/marker via
   `db exec "UPDATE parser_rules SET … WHERE source_id='<id>'"`; re-verify until all
   *required* fields OK (`not-required` for the product_kind doesn't count).
6. **Dry-run parse** — `parse capture <capture_id> --source <id> --dry-run`; eyeball
   dates/nights/price_per_person/airline/hotel. **Cross-check the flight number** against
   known flights (CI120/CI121) — `flight_rx` false-positives are real (risk 6d).
7. **Live parse** — `parse capture <capture_id> --source <id>`; expect `imported>=1`.
8. **Confirm in DB** — `query-offers --plan-id okinawa-2026 --dest okinawa_2026` shows the
   offer with today's `scraped_at`.
9. **Stamp the rule** — `UPDATE parser_rules SET source_url='<live-url>', fetched_at='<now>'`.
10. **settour only** — set `has_custom_parser=1` (see §5a) and re-run 6–8 through the Rust path.
11. **Decommission gate (§4)** passes → `rm archive/broken-python-scrapers/<source>*.py`.

## 4. Definition of done / decommission gate (per source)

A source is live-verified (and its Python parser deletable) when ALL hold, by command output.
**All of G1–G4 must run against the SAME `capture_id`, and that capture must belong to the
source** (see G0 — Codex review caught that `parse capture`/`verify` do NOT enforce this).

- **G0 capture↔source match** — the capture used for G1–G4 has `captures.source_id == <id>`.
  This is NOT enforced by the tools today: `parse capture` binds the stored source as
  `_stored_source` and ignores it, trusting the CLI `--source` (`main.rs:1772`); `verify` only
  prints `warning capture_source_mismatch` (`main.rs:1870`), it does not fail. So a `settour`
  capture parsed `--source liontravel` silently runs liontravel's rules and could fake a
  "verified". Until §5h hardens this in code, G0 is a MANUAL check:
  `db query "SELECT source_id FROM captures WHERE capture_id='<cap>'"` must equal `<id>`.
- **G1 live capture** — latest `captures` row for the source has `captured_at >= today`,
  `LENGTH(raw_text) > 500`, AND its stored `url` host/path is the real OTA results/product page
  for the target trip (not a login wall, generic landing page, or wrong-date page). `raw_text >
  500` alone is insufficient — a login wall easily exceeds it. Eyeball the capture `url` +
  `verify` snippet to confirm it's the right page.
- **G2 verify clean** — `verify` emits zero `MISSING` lines for *required* fields; `overall OK`.
- **G3 parse ≥1** — `parse … --dry-run` prints `offers N` (N≥1) with plausible values
  (date `2026-0X-XX`; price TWD 5k–50k package / 1k–10k flight; valid flight/hotel).
- **G4 offer in Turso** — `query-offers` shows ≥1 row, `source_id=<id>`, `scraped_at>=today`.
- **G5 rule stamped** — `source_url` points at the real OTA URL (not the dead
  `repo:scripts/...` placeholder); `fetched_at` post-2026-06-24.
- **G6 deletion hygiene** — before `rm`, grep the repo: no live command, doc, test, skill, or
  registry entry still references the archived parser file as an executable source
  (`grep -rn "<source>.py" --include=*.rs --include=*.md --include=*.py .` outside `archive/`).

Only then: delete the archived Python file.

## 5. Code changes (vs operational/data work)

Most of the migration is **human-browser + data tuning**, not code. The code deltas:

- **5a (CRITICAL) settour flag** — `default_parser_rules()` `main.rs:1577`: flip
  `has_custom_parser: false` → `true`, then `parser rules seed-defaults` to upsert. (Code fix
  is authoritative; a bare `UPDATE` gets clobbered by the next seed.) Unblocks settour G2–G3.
- **5b `content_snippet`** `main.rs:1116` — the needle list is **okinawa-2026-hardcoded**
  (`HOTEL`/`AZAT`/`6/12`/`Naha`/`那霸`/`China Airlines` are already there — Codex caught that my
  earlier "add 那霸" was wrong; it's present at `main.rs:1123`). Real fix: generalize the
  trip-specific needles (don't bake one trip's hotel/date into the binary) and add the missing
  generic flight markers (`TPE`, `出發日期`, `機票`, `OKA`) so `verify` snippets surface on
  flight-source captures too.
- **5c `infer_source_id`** `main.rs:1091` — recognize `trip.com`, `eztravel.com.tw` (today
  they fall to `unknown` on a bare `browser snapshot`).
- **5d `source_url` seeds** `default_parser_rules()` — change dead
  `Some("repo:scripts/...")` → `None` so a `seed-defaults` re-run can't clobber a verified URL
  back to a dead path.
- **5e (verify-only, no change)** confirm `parse_settour`'s helpers (`extract_date_range`,
  `extract_nights`, `extract_flight_numbers`) are defined/reachable before wiring 5a.
- **5f (OPTIONAL) `chromeport verify-all`** — new `Command::VerifyAll` looping every
  `parser_rules` source against its latest capture. Convenience audit; build *after* Tier A.
- **5g (no change) travel-cli** — `parse capture` writes the shared `offers` table directly;
  `query-offers` already reads it. No travel-cli edits.
- **5h (NEW — from Codex GAP) enforce capture↔source match** — today `parse capture` binds the
  stored source as `_stored_source` and ignores it (`main.rs:1772`), trusting CLI `--source`;
  `verify` only prints `warning capture_source_mismatch` (`main.rs:1870`). A capture can be
  parsed/verified under the WRONG source and fake a "verified", undermining the whole
  decommission gate. Change both to **FAIL** (`CliError`/non-zero exit) when
  `stored_source != source_id`, with an explicit `--allow-source-override` escape hatch for the
  rare intentional case (e.g. re-parsing an `unknown`-sourced snapshot). Small, mechanical,
  well-scoped. This makes G0 automatic instead of a manual check.

**Possible structural change, gated on Tier B findings (risk 6a):** flight-results pages are
tables; `body.innerText` flattens them. chromeport already parses `<table>` DOM into
`TravelCapture.tables` but does **not** persist it. If generic regex fails on flight sources,
extend `turso.rs:insert_capture` to store a denormalized table rendering (NOT JSON in the RDB
— a text rendering or child rows) and let `parse_generic` consume it. Decide only if a flight
source actually fails — don't build it speculatively.

## 6. Sequencing (code-vs-human split)

- **Tier A — FIT packages** (`product_kind=fit`, richest data, shared ZH markers): settour →
  liontravel → besttour → lifetour → travel4u. settour first (only code fix is 5a; parser
  already written). One browser session can batch all five; tuning one informs the rest.
- **Tier B — flight-only** (`product_kind=flight`): tigerair → google_flights → eztravel →
  trip. Simpler structure but bot-detection/login risk; may need a separate session and the
  table-capture change (6a). trip/tigerair: confirm logged-out price renders.
- **Tier C — hotel**: agoda (`product_kind=hotel`); expect `hotel_name_rx` tuning. No login.
- **Blocked/deferred**: skyscanner (captcha — permanently blocked, no automated path);
  booking/jalan/rakuten_travel (inactive/unsupported — no work).

Code work (5a–5d) is a small, single batch done up front. Everything else is per-source
human-browser capture + regex tuning, parallelizable across sources within a session.

## 7. Risks / honest gaps

- **6a structure loss** — `body.innerText` flattens flight tables; may need the table-capture
  change for Tier B (gated, see §5).
- **6b ZH markers drift** — seeded markers came from the *dead Python parsers*, not live
  renders; `verify` surfaces mismatches immediately as `price MISSING` + snippet. Expected.
- **6c pax math** — all `pax_divisor=2`; the search form MUST be filled with 2 adults or
  per-person price is wrong. Operational discipline.
- **6d flight_rx false positives** — `[A-Z]{2}\d{3,4}` matches product/room codes too; always
  cross-check the dry-run flight against CI120/CI121; constrain with `airline_rx` if needed.
- **6e login walls** — tigerair confirmation / trip detailed prices may need auth; confirm the
  results-page level is logged-out-accessible before capture.
- **6f churn** — regex vs rendered text is brittle; a 0-offer parse IS a loud error
  (`Err("parser produced 0 offers")`), but there's no scheduled re-verify — fold
  `check-freshness` into `/pre-trip-checklist`.
- **6g offers PK** — `ON CONFLICT(id, scraped_at) DO NOTHING` can silently drop a same-second
  re-capture (`imported=0`, no error); acceptable, not a blocker.

## 8. Process

1. (this doc) plan written.
2. **Codex CLI reviews this plan** (read-only); I **corroborate** each finding vs the real
   chromeport source before accepting.
3. If warranted, **Grok 3rd-reviewer** pass + corroborate.
4. **Appraise which agent implements which batch** — the small code batch (5a–5d) is
   well-scoped/mechanical (Grok-suitable); per-source capture+tune is human-in-the-loop
   (you drive Chrome, agent runs chromeport + reads verify output).
5. I review any delegated code line-by-line BEFORE commit (logged lesson).
