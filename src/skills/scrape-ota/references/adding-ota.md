# Adding New OTA Support

Step-by-step guide to register a new OTA for the gwebcdb capture + agent `ota write-offers` pipeline.
(There are no Python parser modules and no `parser_rules` parse path. The Python scrapers are decommissioned.)

## Steps

1. Add an entry to the `ota_sources` table in Turso:
   ```bash
   ./bin/travel db exec "INSERT OR IGNORE INTO ota_sources (source_id, name, status, url_template) VALUES ('new_ota', 'New OTA', 'active', 'https://...')"
   ```
   Fields:
   - `source_id`: unique snake_case identifier (used by gwebcdb capture and `ota write-offers`)
   - `name`: display name
   - `status`: `active` once a live capture path exists
   - `url_template`: base/listing URL for the source

2. Verify the source has enough registry metadata for queueing/capture. Extraction is agent-first:
   no `parser_rules`, no custom parser, and no in-CLI parse step.

3. Capture a real page once to land a plain-text capture in the `captures` table:
   ```bash
   # from ~/b/gwebcdb, after exporting TURSO_URL/TURSO_TOKEN
   python bridge/navigate.py "<url>"
   python bridge/ota_capture.py --source new_ota [--url-contains <substr>]   # → capture_id
   ```

4. Read `captures.raw_text` and extract the decision-relevant offer fields:
   ```bash
   ./bin/travel db exec "SELECT raw_text FROM captures WHERE capture_id='<capture_id>'"
   ```

5. Emit TSV and write the offers into Turso:
   ```bash
   ./rust/target/debug/travel ota write-offers <job_id> --capture <capture_id> --claim-token <token> --tsv <path>
   ./bin/travel query-offers --source new_ota
   ```

6. Mark `supported=1` in `ota_sources` only once the live capture + agent-write path works
   end-to-end against a real page.

## Notes

- Tune extraction by rereading `captures.raw_text` and regenerating the TSV, not by editing parser rules.
- gwebcdb attaches to WSLg Chrome on `127.0.0.1:9222`; it does not launch its own browser.
- Set `rate_limit` conservatively for a new OTA.
