# Adding New OTA Support

Step-by-step guide to register a new OTA for the chromeport CDP capture pipeline.
(There are no Python parser modules — parsing is rule-driven from the `parser_rules`
Turso table. The Python scrapers are decommissioned.)

## Steps

1. Add an entry to the `ota_sources` table in Turso:
   ```bash
   ./bin/travel db exec "INSERT OR IGNORE INTO ota_sources (source_id, name, status, url_template) VALUES ('new_ota', 'New OTA', 'active', 'https://...')"
   ```
   Fields:
   - `source_id`: unique snake_case identifier (used as `--source <id>` for chromeport)
   - `name`: display name
   - `status`: `active` once a live capture path exists
   - `url_template`: base/listing URL for the source

2. Add parse rules for the source in the `parser_rules` Turso table (one row per field the
   parser should extract). These rules drive `chromeport parse capture` — no code change:
   ```bash
   ./bin/travel db exec "INSERT INTO parser_rules (source_id, field, selector, ...) VALUES ('new_ota', 'price', '...', ...)"
   ```
   For a custom (non-generic) parser, set `has_custom_parser=1` on the source and provide the
   flight/hotel-specific rule shape the generic parser requires.

3. Capture a real page once to land a plain-text capture in the `captures` table:
   ```bash
   ./rust/target/debug/chromeport fetch interact "<url>" --source new_ota --step 'click:SEL' --step 'fill:SEL=VALUE'
   ```
   (or `browser snapshot --page <N> --source new_ota` if you navigated the tab manually).

4. Run a read-only diagnostic to see what the rules will match before writing offers:
   ```bash
   ./rust/target/debug/chromeport verify new_ota <capture-id>
   ```

5. Parse the capture into the Turso `offers` table and iterate on the `parser_rules` rows
   until the extracted fields are correct:
   ```bash
   ./rust/target/debug/chromeport parse capture <capture-id> --source new_ota
   ./bin/travel query-offers --source new_ota
   ```

6. Mark `supported=1` in `ota_sources` only once the live capture + parse path works
   end-to-end against a real page.

## Notes

- Tune extraction by editing `parser_rules` rows, not code — re-run `parse capture` to retest.
- The driver attaches to a real Chrome on `127.0.0.1:9222`; it does not launch its own browser.
- Set `rate_limit` conservatively for a new OTA.
