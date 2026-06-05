# Rust CDP Scraper Migration Plan

**Status:** Proposed  
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
