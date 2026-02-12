---
name: new-destination
description: Add new destination to configuration with validation to prevent runtime errors
version: 1.0.0
requires_skills: [travel-shared]
requires_processes: []
provides_processes: []
---

# /new-destination

## Purpose

Add new destination to system configuration with proper validation to prevent:
- Missing destination config at runtime
- Weather fetch failures (destination not found)
- OTA scraper region mismatches
- Dashboard deployment errors

## When to Use

Run when:
- Planning a new trip to a destination not in `destinations.json`
- Adding a combined region (e.g., osaka_kyoto, tokyo_yokohama)
- Splitting an existing region into separate destinations

## Workflow

### 1. Check existing destinations

```bash
cat data/destinations.json | jq '.destinations | keys'
```

**Example output**:
```json
[
  "tokyo_2026",
  "nagoya_2026",
  "osaka_2026",
  "osaka_kyoto_2026"
]
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

**Example**: Adding standalone Kyoto
```json
{
  "slug": "kyoto_2026",
  "display_name": "Kyoto",
  "ref_id": "kyoto",
  "ref_path": "src/skills/travel-shared/references/destinations/kyoto.json",
  "timezone": "Asia/Tokyo",
  "currency": "JPY",
  "markets": ["TW", "JP"],
  "primary_airports": ["KIX", "ITM"],
  "language": "ja",
  "origin": "taiwan",
  "coordinates": { "lat": 35.0116, "lon": 135.7681 }
}
```

### 3. Add to destinations.json

```bash
# Edit data/destinations.json
# Add new entry under "destinations" object
```

**Validation**:
- Slug must be unique
- Coordinates must be valid (lat: -90 to 90, lon: -180 to 180)
- Timezone must be valid IANA format
- Currency must be ISO 4217 code

### 4. Create destination reference file

```bash
# Create POI/cluster data file
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

### 5. Create plan in Turso (optional)

```bash
npm run db:seed:plans
```

**Purpose**: Initialize destination in cloud database for cross-device sync

### 6. Add ZH content (optional)

```bash
# Create Chinese translation for dashboard
touch data/<slug>-trip-plan-zh.md
```

### 7. Verify configuration

```bash
# Test destination loads
npm run view:status -- --dest <slug>

# Should show:
# Destination: <display_name>
# Status: All processes pending (expected for new destination)
```

### 8. Test weather fetch

```bash
# Verify coordinates work with weather API
npm run travel -- fetch-weather --dest <slug>

# Should fetch without "destination not found" error
```

## Validation Checklist

```
□ Slug is unique (not in existing destinations)
□ Display name is human-readable
□ Timezone is valid IANA format
□ Currency is valid ISO code
□ Coordinates are valid (lat/lon)
□ Primary airports are valid IATA codes
□ Reference file created (if using POI data)
□ destinations.json is valid JSON (no syntax errors)
□ Weather fetch works
□ view:status shows destination
```

## Common Issues

### Issue: "Destination not found" when fetching weather

**Cause**: Slug in command doesn't match slug in destinations.json

**Fix**: Check exact slug spelling (case-sensitive)

### Issue: Weather API returns wrong location

**Cause**: Coordinates are incorrect

**Fix**: Verify coordinates on Google Maps or OpenStreetMap

### Issue: OTA scrapers return wrong region

**Cause**: OTA region codes don't match destination

**Fix**: Update `data/ota-sources.json` with correct region mappings

### Issue: Dashboard doesn't show destination

**Cause**: Turso database not synced

**Fix**: Run `npm run db:sync:destinations`

## Quick Command

```bash
# Validate destinations.json syntax
cat data/destinations.json | jq '.' > /dev/null && echo "✅ Valid JSON" || echo "❌ Syntax error"

# List all destinations
cat data/destinations.json | jq -r '.destinations | keys[]'

# Check specific destination
cat data/destinations.json | jq '.destinations.<slug>'
```

## Example: Adding Kyoto as Standalone

```bash
# 1. Check it doesn't exist
cat data/destinations.json | jq '.destinations.kyoto_2026'
# null (good, doesn't exist)

# 2. Add to destinations.json
# (Edit file manually)

# 3. Create reference file
cat > src/skills/travel-shared/references/destinations/kyoto.json << 'EOF'
{
  "destination": "kyoto",
  "display_name": "Kyoto",
  "areas": {
    "central": ["Kyoto Station"],
    "east": ["Gion", "Higashiyama"]
  },
  "clusters": {},
  "pois": {}
}
EOF

# 4. Verify
npm run view:status -- --dest kyoto_2026

# 5. Test weather
npm run travel -- fetch-weather --dest kyoto_2026
```

## Integration with Other Skills

- **Before**: Check if destination already exists (avoid duplicates)
- **After**: `/p1-dates` (set dates for new destination)
- **After**: `/p2-destination` (configure destination details)
- **Related**: `/weather-update` (test weather fetch)

## See Also

- `data/destinations.json` — Destination registry
- `src/skills/travel-shared/references/destinations/` — POI data
- `data/ota-sources.json` — OTA region mappings
- `/weather-update` — Weather fetch validation
