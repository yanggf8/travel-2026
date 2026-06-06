# Broken Python scrapers — DECOMMISSIONED, DO NOT RUN

These Python scrapers are archived here because their URL/region/template
construction **404s or lands on the wrong page** (e.g. settour `tour.settour.com.tw/product/<code>`
is 404; the real FIT lives at `fit.settour.com.tw/product/v2` with dynamic `regionId` state the
headless templates never acquire).

**Replacement:** the Rust CDP driver `rust/crates/travel-scraper` — drive the real OTA page in
Chrome (`scrape interact` / `browser snapshot`), then `parse capture <id>` (rule-driven via the
Turso `parser_rules` table) → Turso. See `docs/plans/2026-06-05-rust-cdp-scraper-migration.md`.

Nothing in the repo references these files. Kept only as historical reference. Do not re-wire them
into `scripts/`, npm, skills, or any runnable path.
