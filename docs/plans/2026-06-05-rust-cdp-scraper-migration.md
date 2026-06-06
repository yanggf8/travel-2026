# Rust CDP Scraper Migration Plan

**Status:** In progress — full pipeline DONE (browser/CDP → capture → interactive click/fill →
rule-driven parser → Turso import), no-JSON/plain-text, native Rust→Turso. 10 OTA `parser_rules`
seeded (all `has_custom_parser=0`); settour + liontravel real-scrape-verified and their Python parsers
deleted. Added `verify <source> <capture-id>` for live regex close-out; current stored real rendered
capture verifies settour only. **Credentials: turso-util tiered token minting is LIVE and verified
(2026-06-07) — `turso auth login` done, `db token-status read` shows source=minted (scoped, 24h
expiry); the static `.env` token path is removed; all Turso-backed tests pass against the live DB via
minted tokens.** Tooling is complete; remaining work is OPERATIONAL: per-OTA live capture →
`verify` → fix regex if needed → delete that OTA's archived Python. Needs the human-in-the-loop
capture (user drives real Chrome) for besttour/lifetour/travel4u and the flight/hotel-only OTAs before
advancing decommission status. Flight/hotel rule shape is implemented, seeded, and snippet-verified;
live capture parity still required per source.
See Progress.  
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
- Added `parse capture <file> --source <id>` — a pure transform after rule lookup (no browser/CDP/Turso
  writes) reading a
  `travel-capture-v1` file → CanonicalOffer[] JSON in the "Format B" shape the existing TS importer
  (`scripts/import-offers-to-turso.ts`) already accepts (`{source_id, package_type, url, scraped_at,
  dates, price, flight, hotel}`).
- **settour parser** (`parse_settour` in `main.rs`): extracts dates, nights, IT-flight numbers,
  total + per-person price, hotel name, `package_type=fit` from the captured raw_text.
- **Golden parity test** (`tests/settour_parity.rs`, `cargo test` ✓): an inline minimal public settour
  capture parses to the exact known-correct values read from the live Turso record
  `settour-fit-kyoto-20260620-4n`:
  6/20→6/24, 4 nights, IT212/IT211, total TWD 36,587 / 18,294 pp, 微笑飯店京都烏丸五條, fit.
- `TravelCapture` now derives `Deserialize` so capture files round-trip back into the parser.
- Note: settour FIT renders ONE cheapest-combo offer per capture; parser emits one offer. Per-person
  is total/2 (the adtCount=2 flow) rounded to nearest. Both grounded in real capture content.

### Rule-driven parser engine — DONE and verified
- Added Turso table `parser_rules` plus `parser rules seed-defaults`. Rules are regex/marker rows
  (`date_range_rx`, `nights_rx`, `price_marker`, `price_amount_rx`, `flight_rx`,
  `hotel_anchor_rx`, `airline_rx`, `hotel_name_rx`, price basis, pax divisor, product kind), not
  per-OTA Rust files.
- `parse capture` now loads the `parser_rules` row for `--source`. `has_custom_parser=0` routes through
  the generic engine; `has_custom_parser=1` is reserved for rare Rust override functions. Missing row,
  invalid regex, or unextractable required fields fail loud.
- Product-kind required fields are now kind-aware:
  - `fit`/`group`: dates, nights, price, hotel; flight optional.
  - `flight`: depart date, price, and flight number or airline; no hotel/nights required.
  - `hotel`: check-in/out dates, nights, price, hotel; no flight required.
- Seeded rows for all 10 OTA sources now use the generic path (`has_custom_parser=0`) unless a future
  real scrape proves a custom override is genuinely required. Provenance is stored as
  `source_url=repo:scripts/scrapers/parsers/<source>.py` or the archived equivalent.
- **Verified generic path:** `cargo test -- --nocapture` parsed settour and liontravel through Turso
  rules and matched live `shaping_tour_group_offers` records:
  - settour: 2026-06-20→2026-06-24, 4 nights, IT212/IT211, TWD 36,587 total / 18,294 pp,
    微笑飯店京都烏丸五條, fit.
  - liontravel: 2026-06-12→2026-06-16, 4 nights, CI120/CI121, TWD 37,108 total / 18,554 pp,
    HOTEL AZAT NAHA, fit.
- This supersedes the old per-OTA "Parser Migration Order" as a code-writing sequence: adding an OTA is
  now a Turso `parser_rules` row plus a parity check. A Rust parser file is only justified when the row
  sets `has_custom_parser=1` because the page is genuinely too irregular for regex/marker rules.

### No-JSON capture→parse→import — DONE
- Pipeline boundary changed from file JSON to Turso rows + plain-text CLI output. `browser snapshot` and
  `scrape` store rendered captures in Turso `captures` (`capture_id`, `source_id`, `url`, `title`,
  `captured_at`, `raw_text`) and print a `capture_id`.
- `parse capture <capture-id> --source <id> [--dry-run]` reads raw text from Turso, loads
  `parser_rules`, parses, and imports directly into `offers`. `--dry-run` prints a tab-separated
  plain-text offer line; no `scrapes/captures/*.json`, no `CanonicalOffer[]` file, no `import offers`
  file boundary.
- `db query` renders tab-separated plain-text tables. JSON remains only as an internal protocol/library
  detail where unavoidable, not a user-facing artifact or source of truth.

### Live capture rule verifier — DONE
- Added `verify <source-id> <capture-id>` as a read-only close-out command. It loads `captures.raw_text`
  from Turso, loads the source's `parser_rules` row, prints every rule regex, then reports each extracted
  field as `OK`, `MISSING`, or `not-required`:
  depart/return, nights, price, flight number, airline, hotel, and an overall parser result. It never
  imports offers.
- Current Turso `captures` table contains 10 rows. Only `settour-test-0620` is a full rendered-page
  capture (2164 chars of actual Settour page text); it verifies cleanly:
  `2026-06-20→2026-06-24`, 4 nights, IT212/IT211, TWD 36,587 total / 18,294 pp,
  微笑飯店京都烏丸五條.
- Other stored rows are snippet/test rows (`*-parity-test`, `*-rule-test`, `*-roundtrip-test`,
  `tigerair-test`). They are useful command/test fixtures but are not live verification evidence.
- No regex changes were needed from current stored captures. `tigerair-test` intentionally fails
  (`dummy flight text`) and is not a real capture.

### OTA parser_rules seed + Python decommission — DONE
- `parser rules seed-defaults` now seeds 10 rows:
  `agoda`, `besttour`, `eztravel`, `google_flights`, `lifetour`, `liontravel`, `settour`, `tigerair`,
  `travel4u`, `trip`.
- Verified generic parser path via Turso-backed tests:
  - `settour`: real Rust+CDP verified; Python parser decommissioned.
  - `liontravel`: real Rust+CDP verified; Python parser decommissioned.
  - `lifetour`: rule verified against live Turso record
    `lifetour-okinawa-20260621-2n-mpnpatpt` using a representative rendered-text capture row
    (`2026-06-21→2026-06-24`, 2 nights, TWD 15,130 pp, 沖繩那霸旭橋托麗芙特酒店). It is not
    decommissioned yet because it still needs a real Rust+CDP scrape gate.
  - `besttour`: representative rendered-text parity test added against
    `besttour-okinawa-20260612-3n-mpnnpolq`; needs real Rust+CDP scrape gate before status advances.
  - `travel4u`: representative rendered-text parity test added against
    `travel4u-okinawa-20260621-3n-mpnp86ot`; needs real Rust+CDP scrape gate before status advances.
  - `tigerair`: flight-only rule shape test added (`product_kind=flight`; no hotel/nights required).
  - `agoda`: hotel-only rule shape test added (`product_kind=hotel`; no flight required).
- `google_flights`, `trip`, and `eztravel` are seeded with flight rules and `has_custom_parser=0`, but
  still need live or representative parity cases before being considered verified.
- Live verification status from Turso captures:
  - `settour`: LIVE-VERIFIED against full rendered capture `settour-test-0620`.
  - `liontravel`: already decommissioned from prior real Rust+CDP verification; no additional full
    rendered capture is currently stored in `captures`.
  - `besttour`, `lifetour`, `travel4u`: snippet-verified only; awaiting live user-driven capture before
    Python/archive deletion status can advance.
  - `tigerair`, `google_flights`, `trip`, `agoda`, `eztravel`: rule-shape/snippet verified where tests
    exist; awaiting live user-driven capture.
- Deleted verified Python parser modules:
  `scripts/scrapers/parsers/settour.py`, `scripts/scrapers/parsers/liontravel.py`.
  `scripts/scrapers/parsers/__init__.py` no longer imports them, `scripts/scrapers/registry.py` no
  longer advertises missing parser modules, and the legacy `scripts/scrape_liontravel_dated.py` is now a
  loud decommission stub.

### Rust→Turso CLI — DONE
- Added a native `libsql` Turso module for `travel-scraper`. It now resolves credentials through the
  vendored `rust/crates/turso-util` broker, using tiered token minting (`read`, `write`, `secrets`) and
  a safe token cache. The code no longer hunts for repo-local `.env` credentials.
- Env overrides are scoped to `TRAVEL_TURSO_<TIER>_TOKEN` plus `TRAVEL_TURSO_URL`; generic shell
  `TURSO_<TIER>_TOKEN` variables are intentionally ignored by this travel CLI so unrelated tokens cannot
  hijack resolution. Default bootstrap is an authenticated Turso CLI (`turso auth login`).
- Added `db query <sql>` and `db exec <sql>` so Rust can inspect/mutate Turso without npm.
- `parse capture` imports directly into `offers` with append-only conflict handling
  (`ON CONFLICT(id, scraped_at) DO NOTHING`) and no JSON file boundary.
- Added `db token-status <read|write|secrets>` as a plain-text credential probe.
- Current local verification note: `db token-status read` resolves through the broker cache
  (`source=cache`, db `travel-2026`). If the cache expires, run `turso auth login` to mint fresh tier
  tokens.

### Remaining (next milestones, in order)
- Real-scrape the newly seeded OTAs, starting with `lifetour`, `besttour`, and `travel4u`; then advance
  status only after the Rust+CDP scrape + Turso parity gate passes.
- Real-scrape flight/hotel-only sources (`tigerair`, `google_flights`, `trip`, `agoda`, `eztravel`) and
  add parity checks. Keep `has_custom_parser=0` unless a real capture proves regex rules cannot express
  the source.
- Python decommission per source: delete-verified-now, port-rest-lazily. Only `settour` + `liontravel`
  are deleted so far.
  - **DECOMMISSION PACE (user, 2026-06):** delete-verified-now, port-rest-lazily. Delete the Python
    parser for an OTA as soon as it's verified by real Rust CDP scrapes (settour + liontravel qualify —
    both real-scraped via live Chrome). Keep the other ~8 Python parsers until each OTA is actually
    scraped through the Rust path. Python shrinks incrementally as OTAs are exercised, not in one bulk
    pass. (~30 Python files remain today; only settour/liontravel are real-scrape-verified so far.)
- Screenshot capture; OTA domain allowlist; profile-guard hardening (currently needs
  `--i-understand-profile` because Chrome wasn't launched with `--enable-automation`).
- Full npm/Python/TS-importer decommission remains a separate tracked migration. TODO: write a dedicated
  "kill npm — pure Rust+Turso" migration plan before removing npm scripts or TypeScript importer code.

### TODO — deferred, tracked (write a dedicated plan for each)

- **Kill npm — pure Rust + Turso end-state.** User goal: NO npm, NO Python, NO JSON files. Migrate
  every `npm run travel` / `scripts/*.ts` Turso path to the Rust binary; delete `package.json`, the TS
  importer, and the npm scripts once parity is proven. (User deferred writing this plan.)

- **Credentials are cloud-native, not a local `.env`.** The Rust scraper now uses vendored
  `turso-util` token minting and no longer reads repo-local `.env` credentials. Remaining TS/dashboard
  paths may still use their existing credential mechanisms until the separate npm-kill migration replaces
  them. Requirement stays: scraper commands must FAIL LOUD (not hunt paths) when credentials are absent.

  **Research finding (2026-06): Turso has NO secrets-vault product** — only DB auth tokens. A vault
  *inside* Turso would be circular anyway (need a token to read the secret that holds the token). The
  cloud-native answer is Turso's **Platform API + token minting**: hold ONE bootstrap credential
  (injected by the runtime — env at process start / OS keychain / CI secret / CF Worker secret binding,
  never a repo `.env`), and mint **short-lived, scoped (read-only/full), expiring** DB tokens at runtime
  (`2w1d30m`-style expiry; read-only tokens where writes aren't needed). `gwebcdb/crates/turso-util`
  already implements this tiered Read/Write resolution with `mint_allowed` — reuse it. So the plan is:
  bootstrap secret from runtime injection → turso-util mints a scoped token → use → expire. Not a vault.

  **DECISION (user, 2026-06): adopt `gwebcdb/crates/turso-util` token-minting — it already exists.**
  This is the chosen credential path, NOT a new design. turso-util is a working broker:
  `resolve_token(cfg, tier, …)` / `resolve_cached_or_mint(...)` with `TokenTier::{Read, Write, Secrets}`,
  shells `turso db tokens create <db> --expiration <e>`, caches the minted token per (db, tier) with a
  permission check (refuses world-readable cache; chmod 600), and reuses until expiry. **Bootstrap = an
  authenticated Turso CLI** (`turso auth login` / operator file), or `TURSO_<TIER>_TOKEN` env override —
  NO static full-access token in a repo `.env`. The travel-2026 Rust binary should depend on turso-util
  (path-dep or vendor) and call `connect(cfg, TokenTier::Read)` for scrapes / `Write` for imports,
  REPLACING the current `.env`-reading `src/turso.rs`. Codex's note that turso-util is "finance-registry
  oriented" — re-evaluate: its `RegistryConfig` is parameterized (db_name_envs, token_envs, config_home),
  so it can be configured for the travel DB; only confirm it's not hard-coding finance specifics before
  vendoring. Current `.env` token is non-expiring + full-access (JWT has no `exp`/`a` claim) — exactly
  what minting replaces.

  **GitHub secrets do NOT fit the local scraper (2026-06 research).** GitHub Actions Secrets and GitHub
  OIDC ("secretless" workload identity) are both **CI-only** — they hand credentials to a GitHub Actions
  *runner*, and don't extend to external machines. Our scraper runs interactively on a developer WSL box
  driving real Chrome — it can never be a headless CI job — so neither reaches it. Conclusion: the
  project splits by runtime:
  - **Local scraper/CLI** (your machine, interactive): bootstrap secret from **OS keychain or injected
    env** + Turso token-minting. No GitHub, no repo `.env`.
  - **Cloudflare Worker** (cloud runtime, the dashboard): **CF Worker secret bindings** (already done via
    `wrangler secret put`); GitHub OIDC is the right option *only* if/when the Worker deploys via GitHub
    Actions (keyless deploy), not for the scraper.

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
