# Rust CDP Scraper Migration Plan

**Status:** In progress — Phase 0 + capture core DONE and verified (see Progress).  
**Decision:** Replace Python OTA scrapers with a Rust scraper CLI.  
**Do not edit `package.json` yet:** preserve the current TS fallback rule until the Rust scraper path is complete and tested.

**Supersedes:** the "Python scrapers stay in `scraper:*` namespace forever" decision in
`docs/plans/2026-06-05-rust-cli-migration.md` (§2 "Stay TypeScript/Python Forever", §4).
That plan keeps Python scrapers; this plan replaces them. The two share the `rust/`
workspace and `turso-util`, and can proceed in parallel (the `travel` CLI binary and the
`travel-scraper` binary are independent).

## Empirical basis (why now — observed 2026-06-05)

A live FIT scan for a 6/13 trip (Okinawa / Fukuoka / Kyoto) exposed exactly the failure
mode this migration targets:

- **Headless launch returns the wrong page.** settour's listing scraper (`chromium.launch(headless=True)`,
  `scripts/scrapers/base.py:193`) returned settour's **Bali/印尼 default landing page** instead of
  Japan results — its region codes (`JX_4`/`JX_5`) were silently ignored.
- **Constructed URLs 404.** The scraper emitted `tour.settour.com.tw/product/<code>` URLs that are
  **404**; the real FIT product lives at `fit.settour.com.tw/product/v2` and only renders correctly
  with the full dynamic state (`regionId=…`) the headless session never acquires.
- **CDP-against-real-Chrome works.** Reading that same `fit.settour.com.tw` SPA through `gwebcdb`'s
  CDP bridge (visible Windows Chrome) returned the **correct, verified offer** (京都 6/20-6/24, Tigerair
  direct, TWD 36,587/2pax) — saved to Turso. This is the topology the plan adopts.

Lesson driving the **capture contract** below: the fix isn't a language port, it's capturing the
*rendered* page (with its dynamic state) from the user's real browser, then parsing that capture —
never re-deriving URLs/params headlessly. settour FIT is the sharpest current failure case; treat it
as a priority parity target alongside the `liontravel` case named in the migration order.

## Goal

Move OTA scraping off WSL-headless Python Playwright entirely.

The replacement should use the `gwebcdb` browser topology:

```text
Rust CLI in WSL
  -> Chrome DevTools Protocol over localhost:9222
  -> visible Windows Chrome with a dedicated travel profile
  -> rendered OTA page capture
  -> Rust parsers
  -> canonical offer JSON
  -> Turso import
```

The important win is **where the browser runs**: real visible Windows Chrome, not headless Chromium launched inside WSL.

## Non-Goals

- Do not add a TypeScript scraper subsystem.
- Do not keep Python as the long-term parser or browser layer.
- Do not make `scrape-current` the only workflow.
- Do not expose the CDP port beyond localhost.
- Do not fuse capture, parse, and import so tightly that scrape failures cannot be replayed offline.

## Proposed Rust Workspace

Add a Rust workspace without changing npm scripts yet:

```text
rust/
  Cargo.toml
  crates/
    travel-scraper/
      Cargo.toml
      src/
        main.rs
        browser/
          mod.rs
          cdp.rs
          launch.rs
          capture.rs
        capture/
          mod.rs
          schema.rs
        parsers/
          mod.rs
          besttour.rs
          liontravel.rs
          lifetour.rs
          settour.rs
          travel4u.rs
        offers/
          mod.rs
          canonical.rs
        commands/
          mod.rs
          browser.rs
          scrape.rs
          parse.rs
          import.rs
      tests/
        fixtures/
```

Build output goes to `./bin/travel-scraper` later, matching the existing Rust migration convention.

## Browser Backend

Use Rust CDP, not Playwright.

Candidate crates:

- `chromiumoxide`: higher-level async CDP client; can connect to an existing browser WebSocket.
- `headless_chrome`: simpler API; also supports connecting to an existing debug WebSocket.

Selection criteria:

- attach to an already-running Chrome exposed at `http://127.0.0.1:9222`
- list existing targets/pages
- navigate an existing tab
- evaluate JavaScript on a tab
- extract `document.body.innerText`
- extract links, tables, and optional HTML
- capture screenshots
- avoid launching browser processes from WSL for normal scraping

## Chrome Launch Model

Borrow the operational model from `gwebcdb`, but make the profile travel-specific:

```text
C:\chrome-profiles\travel-browser
```

Commands:

```bash
travel-scraper browser launch
travel-scraper browser doctor
travel-scraper browser pages
travel-scraper browser open <url>
travel-scraper browser snapshot --page 0
```

`browser launch` should trigger a Windows scheduled task so Chrome opens in the visible RDP/console session, not WSL Session 0.

`browser doctor` should check:

- `127.0.0.1:9222/json/version` reachable
- DevTools WebSocket URL discoverable
- at least one page exists
- page URL/title visible
- CDP endpoint is localhost
- warning if page domain does not match `--source`

## Capture Contract

Every browser run writes a replayable capture file before parsing:

```json
{
  "schema": "travel-capture-v1",
  "source_id": "liontravel",
  "captured_at": "2026-06-05T00:00:00Z",
  "url": "https://...",
  "title": "...",
  "raw_text": "...",
  "links": [
    { "text": "...", "href": "https://..." }
  ],
  "tables": [
    [["Header"], ["Cell"]]
  ],
  "html": null,
  "screenshot_path": "scrapes/captures/..."
}
```

Parsing consumes capture files. This preserves offline tests and debugging while removing Python.

## Scrape Commands

Primary workflows:

```bash
travel-scraper scrape url "<url>" --source liontravel --dest kansai --out scrapes/
travel-scraper scrape listing --source travel4u --dest kansai --out scrapes/
```

Manual fallback:

```bash
travel-scraper scrape current --source skyscanner --dest kansai --page 0 --out scrapes/
```

`scrape current` is for CAPTCHA, modals, or manually configured JS pages. It should require explicit `--page` unless there is exactly one open page.

Replay/parsing workflows:

```bash
travel-scraper parse capture scrapes/captures/liontravel-*.json --out scrapes/offers/
travel-scraper import offers scrapes/offers/liontravel-*.json --dest kansai
```

The first milestone may call the existing TS Turso importer. Later, `travel-scraper import` can use Rust Turso code.

## Parser Migration Order

Do not port everything at once.

1. `liontravel`: representative JS-heavy failure case.
2. `travel4u`: group tour listing/detail coverage.
3. `besttour`: calendar pricing.
4. `lifetour`: package detail extraction.
5. `settour`: listing + detail extraction.
6. Flight/hotel-only sources: `tigerair`, `google_flights`, `trip`, `agoda`, `eztravel`.

Each parser must be pure:

```text
Capture -> CanonicalOffer[]
```

No parser should directly control the browser or write Turso.

## Python Decommission Policy

Python remains only as legacy until parity is proven.

For each OTA:

1. save at least one sanitized capture fixture
2. implement Rust parser
3. compare Rust output against current accepted import shape
4. import to Turso in dry-run or test mode
5. mark Python parser deprecated for that source
6. remove Python source only after two successful real scrapes

Final removal includes:

- `scripts/scrape_package.py`
- `scripts/scrape_listings.py`
- `scripts/scrape_batch.py`
- `scripts/scrapers/`
- `tests/scrapers/`
- Python scraper references in skills/docs
- `scraper:setup`, `scraper:doctor`, `scraper:batch`, `scraper:pipeline` npm aliases, after Rust binaries are wired

## Minimum Viable Milestone

Build `travel-scraper` with:

- `browser doctor`
- `browser pages`
- `browser snapshot --page N`
- `scrape current --source liontravel --dest kansai`
- Rust `liontravel` parser
- canonical offer JSON output

Success criteria:

- visible Windows Chrome page is captured from WSL over CDP
- no Python code is invoked
- capture artifact is saved
- parser emits at least one canonical offer or a structured failure
- the offer JSON can be consumed by the existing Turso import path

## Progress (verified)

### Phase 0 — browser discovery — DONE (commit, pre-`8cdd005`)
- `rust/crates/travel-scraper` workspace; dependency-free CLI (raw `std::net::TcpStream` HTTP to the
  CDP `/json/*` REST endpoints).
- `browser doctor` + `browser pages` verified live against the user's real Windows Chrome
  (Chrome/148 @ `127.0.0.1:9222`); `browser pages` listed the user's actual open OTA tab.
- `package.json` untouched; gwebcdb untouched; `bin/` + `rust/target/` gitignored.

### Capture core — DONE and verified (commit `8cdd005`)
- Added `chromiumoxide` + `tokio` + serde/chrono for the **CDP WebSocket** path (the REST `/json/*`
  endpoints can list targets but cannot read rendered content). REST doctor/pages preserved as-is.
- `browser snapshot --page N` and `scrape url "<url>"` emit a `travel-capture-v1` file
  (raw_text = body.innerText, links, tables) under `scrapes/captures/` (gitignored raw landing zone).
- **Verified:** capture read 4,669 chars of real rendered SPA content from the live LionTravel page —
  proving the core thesis (Rust+CDP reads what headless WSL Chromium cannot).

### Lessons that refined the design
1. **Public quote page ≠ authenticated order page.** The LionTravel product-detail URL
   (`/detail/<id>?FromDate=…&Days=5`) is a client-side **quote** page that resets to its `2天1夜`
   default — URL params do NOT pin the multi-night state. The real booked detail (hotel/airline/price)
   lives on the **logged-in order page** (`member.liontravel.com/order/myorderlist`). Capturing search
   offers therefore needs **interactive click/fill** to set dates on the SPA, not URL params — this
   promotes the deferred click/fill TODO to the next real milestone.
2. **Authenticated capture flow works and is safe.** User logs into the OTA manually in the dedicated
   automation Chrome (`:9222`); the tool reads only the already-rendered page. For sensitive order
   pages, extract **facts only → Turso** and do NOT persist raw order text to disk (it holds name/
   contact). This kept the no-local-data + privacy boundary intact end-to-end.
3. **Headless was wrong on specifics, not just missing.** For the booked Okinawa FIT, headless scraping
   reported the wrong airline (EVA vs 中華), wrong hotel (Smile vs AZAT), and wrong price (30,714 vs the
   real 37,108). Only the CDP read of the authenticated session was correct — concrete justification
   for the migration.

### Interactive click/fill + DOM capture — DONE and verified
- Added optional `--html` capture for `browser snapshot`, `scrape url`, and `scrape interact`.
  HTML stays off by default because rendered DOM can be large and sensitive.
- Added `scrape interact <url> --source <id> --step ...`: ordered `click`, `fill`, `wait`, and
  bounded `waitfor` steps. Step failures are loud and include the step index plus selector.
- Added an interactive profile guard. Mutating commands require the dedicated automation profile to be
  confirmed from Chrome command-line data, or an explicit `--i-understand-profile` override.
- **Verified on settour FIT:** selector-driven calendar interaction changed the rendered offer from
  `2026/06/08-2026/06/09` (1 night, IT210/IT211, 機加酒未稅總價 `TWD 23,359`/2pax) to
  `2026/06/20-2026/06/24` (4 nights, IT212/IT211, 機加酒未稅總價 `TWD 36,587`/2pax ≈ 18,294 pp), then
  captured the updated `travel-capture-v1` artifact from the real Windows Chrome. The URL `depDate`
  was identical in both captures — the rendered offer changed only because of the interaction, not the
  URL params. (Figures read from the capture files; the 6/20 offer matches the verified Turso record
  `settour-fit-kyoto-20260620-4n`.)

### Parser stage — settour DONE and verified
- Added `parse capture <file> --source <id>` — a PURE transform (no browser/CDP/Turso) reading a
  `travel-capture-v1` file → CanonicalOffer[] JSON in the "Format B" shape the existing TS importer
  (`scripts/import-offers-to-turso.ts`) already accepts (`{source_id, package_type, url, scraped_at,
  dates, price, flight, hotel}`).
- **settour parser** (`parse_settour` in `main.rs`): extracts dates, nights, IT-flight numbers,
  total + per-person price, hotel name, `package_type=fit` from the captured raw_text.
- **Golden-file parity test** (`tests/settour_parity.rs`, `cargo test` ✓): the fixture
  `tests/fixtures/settour-kyoto-20260620-capture.json` (html stripped; public search page, no PII)
  parses to the exact known-correct values of the Turso record `settour-fit-kyoto-20260620-4n`:
  6/20→6/24, 4 nights, IT212/IT211, total TWD 36,587 / 18,294 pp, 微笑飯店京都烏丸五條, fit.
- `TravelCapture` now derives `Deserialize` so capture files round-trip back into the parser.
- Note: settour FIT renders ONE cheapest-combo offer per capture; parser emits one offer. Per-person
  is total/2 (the adtCount=2 flow) rounded to nearest. Both grounded in real capture content.

### Remaining (next milestones, in order)
- `liontravel` parser (then per Parser Migration Order: travel4u, besttour, lifetour, flight/hotel
  sources). Same pattern: capture fixture → parser → golden-file parity test.
- Python decommission per source: only after ≥2 successful real scrapes + parity (plan policy). No
  Python deleted yet.
- Screenshot capture; OTA domain allowlist; profile-guard hardening (currently needs
  `--i-understand-profile` because Chrome wasn't launched with `--enable-automation`); Rust Turso
  import (optional — TS importer stays until parser core proven).

### TODO — deferred, tracked (write a dedicated plan for each)

- **Kill npm — pure Rust + Turso end-state.** User goal: NO npm, NO Python, NO JSON files. Migrate
  every `npm run travel` / `scripts/*.ts` Turso path to the Rust binary; delete `package.json`, the TS
  importer, and the npm scripts once parity is proven. (User deferred writing this plan.)

- **Credentials are cloud-native, not a local `.env`.** This is a cloud-based (Turso) project, yet
  Turso creds (`TURSO_URL`/`TURSO_TOKEN`) are read from a **local `.env` file** in ~10 places
  (`rust/.../turso.rs`, both Rust tests, `main.rs`, `scripts/turso-pipeline.ts`,
  `scripts/import-offers-to-turso.ts`, other TS scripts, `workers/trip-dashboard/src/turso.ts`). That
  local-file dependency contradicts the no-local-data direction and already caused breakage (the
  `db query` CWD failure; the `../../.env` path walk-up in the test). Requirement: stop depending on a
  local `.env` as the source of cred truth — resolve credentials in a cloud-native way (e.g. a single
  documented env-injection at process start, an OS keychain/secret store, or a token broker like
  `gwebcdb/crates/turso-util`'s tiered resolution), so no command needs to locate a repo-relative file.
  Decide the mechanism in its own plan; until then, env-var injection at the shell is the interim
  contract, and code must FAIL LOUD (not hunt paths) when creds are absent.

## Risks

- CDP crates may lag Chrome protocol changes.
- Screenshots/raw HTML can contain sensitive data; artifacts stay under `scrapes/` and must remain gitignored.
- `scrape current` is human-state-dependent; it is a fallback, not the normal batch path.
- Rewriting parsers loses edge cases. Fixture parity is mandatory.
- Rust Turso import should not block browser/parser replacement; use existing import path until the scraper core works.

## First Implementation Step

Create the Rust workspace and implement only browser discovery:

```bash
travel-scraper browser doctor
travel-scraper browser pages
```

This proves Rust can talk to the Windows Chrome CDP endpoint before parser work starts.
