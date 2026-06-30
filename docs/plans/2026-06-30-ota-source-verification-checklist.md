# OTA Source Verification Checklist (agent-first `write-offers` path)

**Created:** 2026-06-30 · **Source of truth:** the `ota_source_coverage` table in Turso
(`./bin/travel db exec "SELECT source_id, product_type, proven, proven_at, method, blocked_reason_code FROM ota_source_coverage ORDER BY proven DESC, source_id"`).
This doc is a human-readable checklist over that table — when you verify a source, update BOTH
(run `./bin/travel set-ota-coverage <src> <ptype> --proven --proven-at YYYY-MM-DD --method agent_parse`
and tick the box here).

## What "verified" means here

Extraction is **agent-first** (the in-CLI regex/custom parser was deleted 2026-06-30, commit
`258e41a` — the coding agent IS the parser). A source is **✅ Rust-verified** only when it has been
taken **live end-to-end through `travel ota write-offers`**:

```
travel ota enqueue <src> <ptype> …            (debug binary: ./rust/target/debug/travel)
travel ota claim --worker <w> --lease-seconds 900   → job_id + claim_token
# from ~/b/gwebcdb (export TURSO_URL/TURSO_TOKEN from travel .env first):
./scripts/start-chrome-cdp-wslg.sh ; python bridge/navigate.py "<results URL>"
# wait ~25s for async price/hotel SPAs; a too-early capture shows price placeholders / "--"
python bridge/ota_capture.py --source <src> [--url-contains <substr>]   → capture_id
# AGENT reads captures.raw_text, extracts offers, emits TSV (type = OFFER KIND package|flight|hotel,
#   NOT the job product_type; per-person WITH-TAX price; real hotel; record the dates the PAGE shows)
travel ota write-offers <job_id> --capture <capture_id> --claim-token <tok> --tsv <path>
```

> ⚠️ **`proven=1` in the DB ≠ Rust-verified.** Six sources were `proven` via the OLD gwebcdb Python
> path (`proven_at=2026-06-26`); only `proven_at=2026-06-30` reflects the new agent-first
> `write-offers` path. The "Rust-verified" column below is the real frontier.

## The checklist

### ✅ Rust-verified end-to-end (`write-offers`, 2026-06-30)
- [x] **settour** (`fit`) — `fit.settour.com.tw/product/v2` (direct GET; per-person `每人機加酒含稅`). 1 combo.
- [x] **eztravel** (`fit`) — `packages.eztravel.com.tw/roundtrip-TPE-<dest>?checkin=…` (GET; SPA ignores the
  GET dates → record the dates the PAGE shows). 10 combos → proved disambiguation in prod.

### ⬜ Proven on OLD path, NOT yet Rust-verified (the actionable queue — agent-parse, no per-source code)
- [ ] **agoda** (`hotel`) — `agoda.com/{hotel_slug}/hotel/{city_slug}-{country}.html?checkIn=…&los=<nights>&adults=…`
- [ ] **besttour** (`group_tour`) — `besttour.com.tw/e_web/search?v=//////<region_id>///////`
- [ ] **google_flights** (`flight`) — `google.com/travel/flights?q=Flights+to+<dest>+from+<origin>+on+<depart>+through+<return>`
- [ ] **travel4u** (`group_tour`) — `travel4u.com.tw/group/area/<area_code>/japan/`

### ⛔ Blocked / deferred (each carries a `blocked_reason_code` — needs a human or is parked)
- [ ] **liontravel** (`fit`) — `renderer_wedge` (Chrome under WSLg wedges on the page; parked)
- [ ] **lifetour** (`group_tour`) — `renderer_wedge` (parked)
- [ ] **booking** (`hotel`) — `cloudflare` (Cloudflare challenge; source `inactive`)
- [ ] **skyscanner** (`flight`) — `captcha` (hard captcha; source `inactive`)
- [ ] **tigerair** (`flight`) — `redundant` (google_flights already carries 台灣虎航; deferred, not blocked)
- [ ] **trip** (`flight`) — `redundant` (deferred)
- [ ] **jalan** (`hotel`) — `unsupported` (source `inactive`)
- [ ] **rakuten_travel** (`hotel`) — `unsupported` (source `inactive`)

## Notes
- A `blocked_reason_code` must come from `coverage_block_reasons` (the catalog lookup). When a
  blocked source is unblocked, clear it via `set-ota-coverage … --proven`.
- Deleting an archived Python parser for a source is gated on that source being ✅ Rust-verified here.
- Full recipe + gotchas: memory `settour-live-verified-agent-parse`; CLAUDE.md "URL Routing".
