# Adding New OTA Support

Step-by-step guide to register a new OTA scraper.

## Steps

1. Add entry to `data/ota-sources.json` (or `ota_sources` table in Turso):
   - `source_id`: unique snake_case identifier
   - `scraper_script`: repo-relative path (e.g., `scripts/scrape_package.py`)
   - `supported`: `true`
   - `rate_limit`: requests per minute

2. Create parser module in `scripts/scrapers/parsers/<ota>.py`:
   - Subclass `BaseScraper`
   - Implement `parse_raw_text()` — pure parsing, no browser (testable without Playwright)
   - Override `prepare_page()` for OTA-specific interactions (tab clicks, form fills, etc.)

3. Register in `scripts/scrapers/registry.py`:
   - Add URL pattern → parser mapping
   - Add `_create_parser` factory entry

4. Export in `scripts/scrapers/parsers/__init__.py`

5. Add tests in `tests/scrapers/test_parsers.py` (pure parsing tests, no Playwright needed)

6. Test end-to-end with a sample URL:
   ```bash
   python scripts/scrape_package.py "<url>" scrapes/<ota>-test.json
   ```

7. Verify output schema matches `ScrapeResult` (see `scripts/scrapers/schema.py`)

## Notes

- Always implement `parse_raw_text()` before `scrape()` — keeps unit tests fast
- Check `base.py` for `navigate_with_retry()` and other browser helpers
- Rate limits are enforced by `BaseScraper` — set conservatively for new OTAs
