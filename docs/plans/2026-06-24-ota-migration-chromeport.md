# OTA migration → gwebcdb (chromeport retired; extraction ported to Python)

**Status:** PLAN v2 — **Phase 0 (the Python port) SHIPPED 2026-06-25** in gwebcdb. Per-source
live capture + decommission remain — **agent-first (escalate-on-block)**, NOT human-in-the-loop:
the agent drives the whole navigate→fill→capture→parse→verify loop autonomously and only WARNS
the user to act on a true blocker (a login wall, a captcha, or Chrome not started on :9222).
**Scope:** Retire `chromeport` entirely. Move the OTA pipeline to **gwebcdb** as the single
WSLg-based entry point: gwebcdb drives the page (it picks WSLg|Windows backend itself) → saves
raw text to Turso → a NEW gwebcdb Python parser turns text → `offers`. Then verify each source
live and delete its archived Python scraper.

> **PROGRESS 2026-06-26 — ALL 3 TYPES PROVEN REAL; G5/G6 done for the 3 proven sources.**
> The per-source sweep is live, driven by the gwebcdb **per-agent Chrome allocator**
> (`bridge/chrome_session.py acquire` — each agent gets an isolated Chrome; Chrome picks the port)
> and the **AGENT-PARSE path** (the coding agent reads the capture `raw_text` and emits offers as
> TSV → `bridge/ota_write_llm_offers.py`; the regex `ota_cli parse` is now the fallback, so the
> gate's G2/G3 regex-verify steps are N/A for agent-parsed sources).
> **SWEEP EFFECTIVELY COMPLETE (2026-06-27): 6 sources PROVEN, 2 blocked, 2 deferred.** Every active
> source is handled. `./bin/travel query-offers --destination tokyo_sep_2026` = 20 real package
> offers across 4 OTAs (eztravel 9, settour 1, besttour 5, travel4u 5), plus flight (google_flights)
> + hotel (agoda) proven separately.
> - ✅ **PROVEN (G1–G6 done):** `google_flights` (flight, 34 offers), `agoda` (hotel, 5), `eztravel`
>   (package FIT, 9), `settour` (package FIT, 1; custom parser kept but agent-parse used — its v2
>   layout mis-reads price/hotel), `besttour` (package group-tour, 5), `travel4u` (package
>   group-tour, 5). All via the AGENT-PARSE path; `parser_rules.source_url` stamped, `scraper_script`
>   nulled in the seed (archived files kept), offers plan-tagged `tokyo_sep_2026`. Commits 69d8362 /
>   da012ef / 4fe0c41.
> - ⛔ **BLOCKED — `liontravel` + `lifetour` (renderer-wedge under WSLg):** their results SPAs hang
>   Chrome's renderer (Playwright attach AND raw-CDP `Runtime.evaluate`/`DOM.getDocument` all hang;
>   closing the tab crashes Chrome; weston crash count does NOT rise → it's the page). Parked — do
>   NOT retry as "needs a flag."
> - ⏸️ **DEFERRED — `tigerair` + `trip` (flight-only, redundant):** the flight TYPE is already proven
>   by google_flights (which carries 台灣虎航 fares among 15 airlines), so these add only single-carrier
>   source-tag duplication at high friction (tigerair = opaque Quasar SPA form, no deep-link; trip =
>   login-wall risk). Documented in the seed notes; revisit only if their direct fares are wanted.
> - **Recipes that worked (for re-runs / new sources):** Package SPAs need FORM-DRIVING, not a GET
>   deep-link. FIT (eztravel/settour): coax the React dest autocomplete (native value-setter +
>   `input`+`keyup`, click the suggestion) → set dates (eztravel `.dpicker__day` by `aria-label`;
>   settour set BOTH flight AND hotel dates or they default to tomorrow) → click 搜尋 → POSTs results
>   XHR → `ota_capture`. Group tours (besttour/travel4u): server-rendered LISTINGS at a numeric
>   region/area code discovered from the homepage (besttour `/e_web/search?v=//////295///////` 東京;
>   travel4u `/group/area/41/japan/` 東京｜東北) — scroll to lazy-load, agent-parse the product rows.
>   The denied-page guard scans host+path+fragment (not the query) so a travel `?checkout=<date>`
>   doesn't wrongly refuse the search (gwebcdb 0957e48).

> **Phase 0 status (DONE).** gwebcdb now owns the extraction pipeline: `bridge/turso_db.py`
> (Turso `/v2/pipeline` client), `bridge/ota_capture.py` (unredacted innerText → `captures`),
> `bridge/ota_parse.py` (generic + settour parsers), `bridge/ota_cli.py` (parse/verify →
> `offers`). Designed + adversarially audited via multi-agent workflows; every wire-format fact
> proven against the live Turso DB; code-reviewed (caught + fixed a real airline-inference parity
> bug). Live-verified on the settour oracle: dates/nights/price/flight/hotel/airline all match the
> retiring Rust. 321 bridge tests pass. **Remaining (NOT Phase 0) — AGENT-FIRST, escalate-on-block:**
> the agent runs the per-source loop autonomously — navigate (`navigate.py`) → fill search
> (`form_fill.py --confirm`) → click Search (`form_click.py --confirm`) → capture (`ota_capture.py`)
> → parse/verify (`ota_cli.py`) → tune the `parser_rules` regex if `verify` flags MISSING → re-run
> → G0–G6 decommission gate → delete the archived Python parser. The agent only WARNS the user to
> act on a genuine blocker: (a) a login wall (`login_assist` / the user signs in once, session
> persists in the profile), or (b) a captcha (skyscanner-class — skip + flag). **The agent CAN start
> Chrome itself** — `bash gwebcdb/scripts/start-chrome-cdp-wslg.sh` (no sudo; WSLg DISPLAY=:0 is
> present) brings up headed Chrome on :9222 in ~3s; it is NOT a human step (an earlier doc wrongly
> said "the agent can't start a display session" — proven false 2026-06-25). The Rust `chromeport`
> binary is superseded but not yet archived.
>
> **Autonomous readiness check (2026-06-25, agent-run against existing captures):** 7/10 sources
> parse cleanly through the new pipeline (settour/liontravel/lifetour/travel4u/tigerair/agoda — all
> on STALE captures needing a refresh); besttour parses-but-needs a `date_range_rx` tune;
> eztravel/google_flights/trip have NO capture yet. So the loop is proven end-to-end on real data —
> it just needs fresh captures, which needs Chrome on :9222.

---

## 1. Corrected architecture (why v1 is obsolete)

**The browser layer is gwebcdb, not chromeport.** You call gwebcdb's bridge tools; gwebcdb's
runtime (`bridge/runtime.py:preferred_backend`) decides the backend — **WSLg-native Chrome
first, Windows Chrome only as fallback** (Windows attach was retired for stability). There is no
separate "chromeport" browser invocation anymore.

**chromeport is being fully retired.** Its browser/CDP half (fetch interact, CdpSession,
settle_rendered_page, screenshot, capture_page) is dropped. Its EXTRACTION half — `parser_rules`
→ generic/settour parsers → `verify` → `parse capture` → `offers` — has **no gwebcdb equivalent**
(gwebcdb has form-driving + raw-text dump but zero structured extraction). So that extraction
logic is **ported into gwebcdb as a Python tool**. End state: one WSLg-based toolset owns the
whole OTA pipeline; the `chromeport` Rust crate is archived.

**Login/2FA is no longer a blocker.** For any OTA that requires sign-in (trip.com prices,
tigerair booking-confirmation views), the human **logs in / settles 2FA by hand** in the WSLg
Chrome window (or via gwebcdb's approval-gated `login_assist` pattern); the session persists in
the dedicated Chrome profile, and gwebcdb's capture tools then read the logged-in results page.
This moves the previously-"deferred, login-walled" sources back into scope.

## 2. Target pipeline

```
You call gwebcdb tools ─▶ gwebcdb runtime picks WSLg|Windows Chrome
   navigate.py / form_fill.py / form_click.py / combo_select.py   (drive the search form;
                                                                    human logs in if needed)
        │
   NEW capture writer (ota_capture.py)  ─▶  Turso `captures`
        │  (UNREDACTED body.innerText + source_id + url + captured_at → capture_id)
        │  ⚠ does NOT reuse save_page_text.py: that tool REDACTS (masks ≥5-digit runs)
        │    BEFORE writing — it would destroy prices (36587) and dates (2026-06-20).
        │
   NEW gwebcdb parser (ota_parse.py — Python port of chromeport extraction)
   parse_capture: read capture + parser_rules → branch has_custom_parser →
                  generic OR settour parser → offer rows → Turso `offers`
   verify_capture: read-only field-by-field OK/MISSING report
        │
        ▼
   Turso `offers`   (then ./bin/travel query-offers reads them — unchanged)
```

gwebcdb stays plain-text/CLI and Turso-only (no JSON files) — same constraints as chromeport.
**Backend nuance (Codex):** `preferred_backend(auto)` returns an ALREADY-RUNNING backend first,
THEN prefers WSLg — so "WSLg-first" holds only when nothing is running. To force WSLg, ensure no
Windows Chrome is up, or pass the WSLg backend explicitly.

## 3. The Python port (the code work)

### 3.0 gwebcdb readiness prerequisites (net-new — MUST land before the parser)

A readiness audit (2026-06-25) found gwebcdb is **architecturally ready but technically PARTIAL** —
three pieces do not exist yet and block the port:

- **P-1 Python Turso access (MISSING).** gwebcdb's Turso path is Rust (`turso-util` + `price-cli`);
  there is NO Python Turso/libsql client (`bridge/requirements.txt` = playwright, pytest,
  faster-whisper only). The port must add one — cleanest is a small `bridge/turso_db.py` hitting
  the Turso `/v2/pipeline` HTTP API with the `.env` token (the same API CLAUDE.md already blesses
  for ad-hoc reads/writes), so no native libsql build is needed. It reads `captures`/`parser_rules`,
  writes `offers`.
- **P-2 Unredacted capture writer (MISSING — and a trap).** `save_page_text.py`/`snapshot.py`
  write LOCAL files, not Turso, AND `save_page_text.py:66` **redacts** the text first
  (`redact_sensitive_numbers`, `common.py:172`: masks any `\d[\d -]{3,}\d` run) — which destroys
  prices (`36587`) and dates (`2026-06-20`). So do NOT reuse it. Build a NEW `bridge/ota_capture.py`:
  attach via the existing CDP runtime, read **raw** `body.innerText`, tag `source_id`, INSERT a
  `captures` row, return `capture_id`. This is the explicit capture boundary chromeport had and
  gwebcdb lacks.
- **P-3 captures/parser_rules/offers schema in the travel Turso DB** — already exist (chromeport
  + travel-cli migrations created them); the Python tools just connect to the SAME travel-2026
  Turso DB. No new schema, but confirm the offers insert matches the live schema (Codex: the live
  `offers` table has MORE nullable/default columns than the 16 chromeport inserts —
  `scripts/schema.sql:516` / `db_migrate.rs:758`; insert the same 16, let the rest default).

Form-driving (navigate/form_fill/form_click/combo_select), the persistent WSLg Chrome profile, the
pytest harness, and the no-JSON house style are all **READY**. `login_assist.py` is E*TRADE-specific —
treat it as a *pattern to copy*, not a generic OTA helper; OTA login = human logs in by hand in the
WSLg Chrome window (session persists in the profile).

### 3.1 The parser itself

Port the **pure text→offers** logic from `chromeport/src/main.rs` + `turso.rs`. The exact
surface (function-by-function, with file:line, signatures, regex/group logic, price math, the
settour char-scanner, the 17-col `parser_rules` + `offers` schemas, helpers) is captured in the
investigation appended below as **Appendix A — port spec**. Build it as a new gwebcdb bridge
module `bridge/ota_parse.py` using the P-1 `turso_db.py`. Key pieces:

- **Generic parser** — 6 extractors: `date_range` (2 groups → YYYY-MM-DD), `nights`
  (group select by `nights_is_days`, offset), `price` (marker-find → `price_amount_rx` →
  `digits_only` → **ceiling div** `(total+divisor-1)/divisor` for `price_basis=total`, multiply
  for `per_person`), `flights` (dedup first/second), `airline` (optional), `hotel`
  (`hotel_name_rx` else anchor → next non-empty line). product_kind branching (CORRECTED, see
  A.3): ONLY flight-kind nulls nights+hotel; flights+airline are ALWAYS extracted (so a hotel page
  can pick up incidental flight data — replicate this); hotel is required for non-flight; flight
  requires a flight# or airline.
- **settour custom parser** — bespoke, no regex: char-by-char `YYYY/MM/DD` window; `共N晚`/`共N日`
  nights; `機加酒未稅總價` marker → first ≥4-digit amount (skip commas); 2-upper+digits flight
  numbers; hotel = line starting `飯店` & containing `入住` → next non-empty line;
  `per_person=(total+1)//2`.
- **Helpers** — `parse_amount`, `digits_only`, `normalize_rule_date`, `compile_rule_regex`,
  `infer_region`/`infer_destination`, `infer_airline_from_flight_number` (IT→虎航, CI→華航, …),
  `offer_to_row`/`offer_row_id` (composite id `source_product_YYYYMMDD_Nn`), `product_code_from_url`,
  `sanitize_id_part`, `now_iso`.
- **The capture↔source guard + `--allow-source-override`** (already proven in Rust) — port it.
- **DROP** all browser/CDP functions (Appendix A §7) — gwebcdb already provides that layer.

**Fidelity gate (expanded per Codex — one capture is NOT enough):** the port must reproduce the
Rust output. The anchor case is known-good: `verify settour settour-test-0620` → `depart_return
2026-06-20→2026-06-24, nights 4, price pp=18294 total=36587, flight IT212/IT211, hotel
微笑飯店京都烏丸五條, overall OK offers=1` (settour custom parser). But add a fixture suite that
exercises the GENERIC parser's branches too — generate each by running the current Rust binary on a
crafted capture and snapshotting its output, then assert the Python matches:
- one generic `fit`, one `flight`, one `hotel` product_kind;
- `price_basis=total` AND `price_basis=per_person`; invalid/zero `pax_divisor` (must error);
- `nights_is_days` true and false;
- flight with a MISSING return date (group 2 None — allowed only for flight);
- hotel via `hotel_name_rx` AND via the anchor→next-non-empty-line fallback;
- the capture↔source guard (mismatch errors; `--allow-source-override` bypasses);
- `offer_row_id` composite-id generation.
Port the Rust unit tests where they exist; otherwise snapshot Rust output as the oracle.

## 4. Per-source migration unit (parallelizable, unchanged in spirit)

Prereqs: WSLg Chrome up (`gwebcdb/scripts/start-chrome-cdp-wslg.sh`), gwebcdb tools resolve a
backend, Turso tokens exported.

1. Rule row exists (seed parser_rules — port the seeder too, or seed via SQL).
2. Navigate the OTA search UI to the target product/results page (human logs in if the OTA
   requires it; set **pax=2**).
3. Capture page text → Turso `captures` via gwebcdb `save_page_text.py` (tagged with source_id).
4. `verify` (Python) — read field OK/MISSING + snippet.
5. Tune `parser_rules` regex/markers on MISSING; re-verify until required fields OK.
6. Dry-run `parse` — eyeball dates/nights/price/airline/hotel; **cross-check flight# vs known
   flights** (false-positive risk).
7. Live `parse` → `offers`.
8. Confirm via `./bin/travel query-offers`.
9. Stamp `parser_rules.source_url` + `fetched_at`.
10. Decommission gate (§5) passes → delete the archived Python scraper.

## 5. Decommission gate (per source) — unchanged from v1, still applies

- **G0 capture↔source match** — now enforced in code (the ported guard).
- **G1 live capture** — `captured_at >= today`, `raw_text > 500`, **and the stored URL is the
  real OTA page** (not a login wall / wrong date).
- **G2 verify clean** — zero MISSING for required fields; overall OK.
- **G3 parse ≥1** — dry-run prints ≥1 plausible offer.
- **G4 offer in Turso** — `query-offers` shows it, today's `scraped_at`.
- **G5 rule stamped** — real `source_url`, fresh `fetched_at`.
- **G6 deletion hygiene** — grep repo: no live ref to the archived parser before `rm`.

## 6. Sequencing

- **Phase 0 (code, blocking) — the Python port** (§3) + fidelity gate vs the known Rust output.
  This is the prerequisite; nothing per-source can be verified until extraction runs in gwebcdb.
- **Tier A — FIT packages**: settour (custom parser, port it faithfully first) → liontravel →
  besttour → lifetour → travel4u.
- **Tier B — flight-only**: tigerair → google_flights → eztravel → trip. Flight result pages are
  tables; raw text flattens them — may need a structured-text capture (gwebcdb `save_page_text`
  vs a table-aware dump). Login: trip/tigerair confirmation may need a human sign-in (now OK).
- **Tier C — hotel**: agoda (expect `hotel_name_rx` tuning). No login.
- **Blocked**: skyscanner (captcha). booking/jalan/rakuten_travel (inactive/unsupported).

Code-vs-human split: Phase 0 port = code (Grok-suitable, tight spec, against a known-good diff).
Per-source capture + tuning = AGENT-driven (the agent runs navigate/form_fill/form_click/
ota_capture/ota_cli autonomously with --confirm/--always-approve); the agent only WARNS the user
to act on a blocker (Chrome-not-started / login wall / captcha).

## 7. Risks / honest gaps

- **Port fidelity** — the settour char-scanner + price ceiling-div + product_kind branching must
  match Rust exactly; the fidelity gate (diff vs `settour-test-0620` output) is the backstop.
- **Structure loss on flight tables** (Tier B) — `body.innerText` flattens; may need a
  table-aware capture in gwebcdb (NOT JSON in the RDB — text rendering or child rows).
- **Regex brittleness / ZH marker drift** — `verify` surfaces it as MISSING + snippet.
- **pax math** — search form must be 2 adults or per-person price is wrong.
- **flight_rx false positives** — `[A-Z]{2}\d{3,4}` matches room/product codes; cross-check.
- **Where chromeport retires to** — archive the crate (like `archive/ts-cli-retired/`); update
  CLAUDE.md/CLI.md/skills that still say `./bin/chromeport`.

## 8. Process
1. (this doc) plan written. 2. Codex CLI reviews → I corroborate vs source. 3. optional Grok 3rd
review + corroborate. 4. appraise agents — Phase 0 port is mechanical-but-large (Grok against the
appended spec + fidelity diff); per-source = agent-driven (escalate-on-block). 5. review delegated code
line-by-line before commit.

---

## Appendix A — chromeport extraction port spec (function-by-function)

> Captured from a read-only investigation of `rust/crates/chromeport/src/{main.rs,turso.rs}` on
> 2026-06-25. This is the authoritative reimplementation reference for §3. Sections: (1) parse
> entry points, (2) generic parser + 6 extractors, (3) product_kind branching, (4) settour
> custom parser, (5) the 3 Turso tables, (6) helper utilities, (7) browser/CDP pieces to DROP.
> [Full spec pasted below at implementation time — see investigation output in session; key
> facts already inlined in §3. The spec gives file:line + signature + logic for every function,
> the 17 parser_rules columns, the 16 offers columns + (id,scraped_at) PK / ON CONFLICT, and the
> exact price/date/nights normalization.]

### A.1 Parse entry points
- `parse_capture(capture_id, source_id, dry_run, allow_source_override)` — main.rs:1780. Steps:
  read `SELECT source_id,url,raw_text FROM captures WHERE capture_id=?`; guard
  `stored_source != source_id && !allow_source_override` → error (main.rs:1803); load
  parser_rules row; branch `has_custom_parser` → `parse_settour` (settour) else `parse_generic`;
  dry_run prints tab-separated offers + `offers\tN`; else `offer_to_row` each →
  `insert_offer` → print `imported\tN`.
- `verify_capture(source_id, capture_id, allow_source_override)` — main.rs:1884. Same read+guard;
  prints the 17-col rule report then per-field `verify_*` OK/MISSING; attempts parse, no write.

### A.2 Generic parser — `parse_generic(capture, rule)` main.rs:2105
Calls 6 extractors then builds one `CanonicalOfferOut`. Skips per product_kind (A.3). Validates:
flight kind with no flight# AND no airline → error (main.rs:2126).
- `extract_rule_date_range` (2188) — `date_range_rx`, groups 1&2; group2 may be None only for
  flight; `normalize_rule_date` each.
- `extract_rule_nights` (2221) — `nights_rx`; group = 1 if `nights_is_days` else last group;
  `digits_only`; if `nights_is_days` subtract 1.
- `extract_rule_price` (2254) — find `price_marker`, slice after it, `price_amount_rx` group1 →
  `parse_amount`; `total` → per_person `(amount+pax_divisor-1)/pax_divisor` (ceiling);
  `per_person` → total `amount*pax_divisor`.
- `extract_rule_flights` (2314) — `flight_rx` captures_iter, group1/0, strip spaces, dedup →
  (first, second).
- `extract_rule_airline` (2368) — empty `airline_rx` → None; else group1/0 trimmed.
- `extract_rule_hotel` (2333) — `hotel_name_rx` if set (group1/0); else `hotel_anchor_rx` →
  first non-empty line after a matching line.

### A.3 product_kind branching (main.rs:2112–2131) — CORRECTED (Codex)
Only `nights` and `hotel` are gated on kind. `flights` and `airline` are **always** extracted
(main.rs:2118-2119, no guard), and `hotel` is **required** (error-propagating `?`) for any
non-flight kind — it is NOT "optional".
| field | fit | flight | hotel | gated? |
|---|---|---|---|---|
| date_range | req | req | req | no |
| nights | req | **None** | req | `kind=="flight"`→None (2114) |
| price | req | req | req | no |
| flights | always | always | always | NONE — extracted unconditionally (2118) |
| airline | always | always | always | NONE — extracted unconditionally (2119) |
| hotel | **req** | None | **req** | `kind=="flight"`→None else required `?` (2120) |
flight validation (2126): error if `flights.0` is None AND `airline` is None.
Consequence: a `hotel`-kind page whose text matches `flight_rx` yields **incidental flight data**
(not skipped). The port must replicate this exactly, including that quirk.

### A.4 settour custom parser — `parse_settour(capture)` main.rs:2612 (no regex)
- `extract_date_range` (2683) — char-by-char sliding 10-char `YYYY/MM/DD` window, `/`→`-`, dedup,
  return first two.
- `extract_nights` (2715) — `capture_after("共","晚")` nights, or `capture_after("共","日")` days−1.
  `capture_after` (2808): substring between markers, ASCII digits only.
- `extract_settour_total` (2726) — find `機加酒未稅總價`, then `extract_first_amount` (2733):
  accumulate ASCII digits, skip commas, return first run ≥4 digits.
- `extract_flight_numbers` (2758) — 2 uppercase + 1–4 digits (len 4–6), dedup → (first, second).
- `extract_settour_hotel` (2790) — line.startswith `飯店` AND contains `入住` → next non-empty line.
- per_person = `(total + 1) / 2` (2 pax). source_id "settour", package_type "fit", TWD.

### A.5 Turso tables
- `captures` (turso.rs:86): capture_id PK, source_id, url, title, captured_at, raw_text.
- `parser_rules` (turso.rs:195) 17 cols: source_id PK, product_kind, date_range_rx, nights_rx,
  nights_is_days, price_marker, price_amount_rx, price_basis, pax_divisor, flight_rx,
  hotel_anchor_rx, airline_rx, hotel_name_rx, currency, has_custom_parser, source_url, fetched_at.
  Upsert ON CONFLICT(source_id) DO UPDATE.
- `offers` — chromeport INSERTS 16 cols (turso.rs:163): id, source_file, source_id, type, name,
  price_per_person, currency, region, destination, departure_date, return_date, nights,
  availability, hotel_name, airline, scraped_at. PK (id, scraped_at), ON CONFLICT DO NOTHING.
  NOTE (Codex, corroborated): the LIVE `offers` table has **21 columns** (`scripts/schema.sql:516`):
  the 16 chromeport inserts + `hotel_area`, `flight_outbound`, `flight_return`, `includes`,
  `created_at` (default). Insert the same 16; the extra 5 take NULL/default — fine. **BUT two of the
  16 are CHECK-constrained**: `type IN ('package','flight','hotel')` and `availability IN
  ('available','sold_out','limited')`. The port MUST emit valid enum values (or NULL for
  availability) — an arbitrary string like `"unknown"` makes the INSERT *fail*, it does not default.
  chromeport's `offer_row_kind` already maps to exactly package/flight/hotel; replicate that.
  `offer_row_id` = `source_id_<product_code>_YYYYMMDD_<N>n`; product_code = last URL path seg
  minus .html/.htm.

### A.6 Helpers to port
`parse_amount`(2416), `digits_only`(2425, ASCII only — handles fullwidth by dropping it),
`normalize_rule_date`(2397, `/`→`-`, validate YYYY-MM-DD), `compile_rule_regex`(2384),
`infer_region`(2575)/`infer_destination`(2566) URL-keyword, `infer_airline_from_flight_number`
(2546: IT→台灣虎航, CI→中華航空, BR→長榮, JX→星宇, MM→樂桃, TR→酷航, GK→捷星),
`offer_to_row`(2469)/`offer_row_kind`(2509, →flight/hotel/package)/`product_code_from_url`(2529),
`non_empty_string`(2586), `sanitize_id_part`(2594), `now_iso`(2816).

### A.7 DROP (browser/CDP — gwebcdb provides this)
CdpSession(665), fetch_url(519), fetch_interact(630), capture_snapshot(489), screenshot_url(553),
settle_rendered_page(768), execute_interaction_steps(792), set_control_value(839),
wait_for_selector(895), capture_page(967), evaluate_{string,links,tables}(1012/1020/1033),
guard_interactive_profile(917). Extraction boundary = raw_text string in.
