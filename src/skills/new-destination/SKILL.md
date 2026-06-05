---
name: new-destination
description: Add new destination to configuration with validation to prevent runtime errors
version: 2.0.0
requires_skills: [travel-shared]
requires_processes: []
provides_processes: []
---

# /new-destination

## Shared references

Read if adding OTA region mappings or destination POI data:
- `../travel-shared/references/ota-registry.md` — OTA source IDs, region codes, scraper scripts
- existing destination reference data (areas/POIs/clusters/transit) — in Turso; inspect a similar one with `npm run travel -- query-destination-ref --slug tokyo_2026` and copy its shape into `scripts/seed-destination-refs.ts`

## Purpose

Add new destination to system configuration with proper validation to prevent:
- Missing destination config at runtime
- Weather fetch failures (destination not found)
- OTA scraper region mismatches
- Dashboard deployment errors

## When to Use

Run when:
- Planning a new trip to a destination not in the DB
- Adding a combined region (e.g., osaka_kyoto, tokyo_yokohama)
- Splitting an existing region into separate destinations

## Workflow

### 1. Check existing destinations

```bash
npm run travel -- query-bookings --dest tokyo_2026  # replace to see DB destinations
npm run view:status
```

Or query the DB directly:

```bash
npx ts-node scripts/turso-exec.ts "SELECT slug, display_name FROM destination_config"
```

### 2. Determine destination details

**Required information**:
- `slug`: Unique identifier (e.g., `kyoto_2026`)
- `display_name`: Human-readable name (e.g., `Kyoto`)
- `ref_id`: Reference ID for POI data (e.g., `kyoto`)
- `timezone`: IANA timezone (e.g., `Asia/Tokyo`)
- `currency`: ISO currency code (e.g., `JPY`)
- `primary_airports`: Airport codes (e.g., `["KIX", "ITM"]`)
- `coordinates`: Lat/lon for weather API

### 3. Add destination to DB via migration

Add an `INSERT OR IGNORE` block to `scripts/turso-migrate.ts` inside the existing destinations backfill loop, then run the migration:

```typescript
// In scripts/turso-migrate.ts — add to the destinations array:
{
  slug: 'kyoto_2026', display_name: 'Kyoto', ref_id: 'kyoto',
  ref_path: '',  // reference data lives in normalized Turso tables, not a file
  timezone: 'Asia/Tokyo', currency: 'JPY',
  markets_json: '["TW","JP"]', primary_airports_json: '["KIX"]',
  language: 'ja', origin: 'taiwan', lat: 35.0116, lon: 135.7681,
},
```

```bash
npm run db:migrate:turso
```

**Validation**:
- Slug must be unique
- Coordinates must be valid (lat: -90 to 90, lon: -180 to 180)
- Timezone must be valid IANA format
- Currency must be ISO 4217 code

### 4. Seed destination reference data into Turso

Reference data (areas, POIs, clusters, transit, tips) lives in the normalized
Turso tables `destination_areas`, `destination_pois`, `destination_clusters`,
`destination_transit`, and `destination_config.tips_json` — **never in a local
JSON file**. Add the new destination's data to the inline `DATA` constant in
`scripts/seed-destination-refs.ts` (keyed by slug), then run it:

```bash
npx ts-node scripts/seed-destination-refs.ts
```

Per-table shape (one row per area / POI / cluster / transit pair):

- **destination_areas**: `area_id, name, type, stations[], vibe, best_for[]`
- **destination_pois**: `poi_id, title, area, nearest_station, duration_min, booking_required, booking_url?, cost_estimate, tags[], notes?, hours?, address?`
- **destination_clusters**: `cluster_id, name, description, pois[], duration_min, best_area`
- **destination_transit**: `pair_key, kind('estimate'|'inter_city'), minutes, line, station_from?, station_to?`
- **tips**: array of strings → `destination_config.tips_json`

Verify with:

```bash
npm run travel -- query-destination-ref --slug <slug>
```

(throws if the destination has no reference rows — fail loud, no file fallback).

### 5. Add destination slug to plan via CLI

After the config row exists in DB, use the standard CLI flow to initialize the plan:

```bash
npm run travel -- set-dates 2026-02-24 2026-02-28 --plan-id kyoto-2026
npm run view:status -- --plan-id kyoto-2026
```

### 6. Verify configuration

```bash
# Should show destination loaded from DB
npm run view:status -- --plan-id kyoto-2026

# Should show: Destination: Kyoto, Status: All processes pending
```

### 7. Test weather fetch

```bash
npm run travel -- fetch-weather --dest kyoto_2026
# Should fetch without "destination not found" error
```

## Validation Checklist

```
□ Slug is unique (not in destination_config table)
□ Display name is human-readable
□ Timezone is valid IANA format
□ Currency is valid ISO code
□ Coordinates are valid (lat/lon)
□ Primary airports are valid IATA codes
□ Reference rows seeded into Turso (if using POI data) — query-destination-ref shows them
□ db:migrate:turso ran successfully
□ Weather fetch works
□ view:status shows destination
```

## Common Issues

### Issue: "Destination not found" when fetching weather

**Cause**: Slug in command doesn't match slug in `destination_config` DB table

**Fix**: Check exact slug spelling (case-sensitive); confirm row exists in DB

### Issue: Weather API returns wrong location

**Cause**: Coordinates are incorrect

**Fix**: Verify coordinates on Google Maps or OpenStreetMap

### Issue: OTA scrapers return wrong region

**Cause**: OTA region codes don't match destination

**Fix**: Update the `ota_sources` table in Turso with correct region mappings (OTA sources stored in DB, not a JSON file)

### Issue: Dashboard doesn't show destination

**Cause**: Plan not initialized in Turso

**Fix**: Run `npm run db:seed:plans` or use CLI to set dates for the new plan

## Integration with Other Skills

- **Before**: Check if destination already exists in DB (avoid duplicates)
- **After**: `/p1-dates` (set dates for new destination)
- **After**: `/p2-destination` (configure destination details)
- **Related**: `/weather-update` (test weather fetch)

## See Also

- `scripts/turso-migrate.ts` — Migration script (source of truth for DB schema and seed data)
- `scripts/schema.sql` — Read-only DDL reference
- `scripts/seed-destination-refs.ts` + `destination_areas`/`destination_pois`/`destination_clusters`/`destination_transit` tables in Turso — POI/area/cluster/transit data (stored in DB, not a JSON file)
- `ota_sources` table in Turso — OTA region mappings (stored in DB, not a JSON file)
- `/weather-update` — Weather fetch validation
