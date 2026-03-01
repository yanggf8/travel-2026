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
- `../travel-shared/references/destinations/` — existing destination JSON files (use as template)

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
  ref_path: 'src/skills/travel-shared/references/destinations/kyoto.json',
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

### 4. Create destination reference file

```bash
touch src/skills/travel-shared/references/destinations/<ref_id>.json
```

**Template**:
```json
{
  "destination": "kyoto",
  "display_name": "Kyoto",
  "areas": {
    "central": ["Kyoto Station", "Downtown"],
    "east": ["Gion", "Higashiyama"],
    "north": ["Kinkaku-ji", "Arashiyama"]
  },
  "clusters": {
    "gion_traditional": {
      "name": "Gion Traditional District",
      "area": "east",
      "pois": ["gion_geisha", "yasaka_shrine", "kiyomizu_temple"]
    }
  },
  "pois": {
    "gion_geisha": {
      "title": "Gion Geisha District",
      "area": "east",
      "nearest_station": "Gion-Shijo",
      "duration_min": 120,
      "booking_required": false,
      "tags": ["culture", "traditional", "photo"]
    }
  }
}
```

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
□ Reference file created (if using POI data)
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
- `src/skills/travel-shared/references/destinations/` — POI data
- `ota_sources` table in Turso — OTA region mappings (stored in DB, not a JSON file)
- `/weather-update` — Weather fetch validation
