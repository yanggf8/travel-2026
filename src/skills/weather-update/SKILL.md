---
name: weather-update
description: Fetch weather forecast and verify destination configuration before fetching
version: 1.0.0
requires_skills: [travel-shared]
requires_processes: [process_1_date_anchor, process_5_daily_itinerary]
provides_processes: []
---

# /weather-update

## Purpose

Fetch weather forecast with pre-checks to prevent common failures (destination not found, itinerary not scaffolded, dates out of range).

## When to Use

After itinerary scaffolded and dates within 16 days of current date.

## Workflow

### 1. Pre-check destination config

```bash
./bin/travel query-bookings --dest all 2>/dev/null || ./bin/travel db exec "SELECT slug FROM destination_config"
```

Common issue: trying to fetch weather for "kyoto" when only "kyoto_2026" exists.

### 2. Pre-check itinerary status

```bash
./bin/travel status --full
```

P5 status must be `researching` or later (not `pending`). If pending, scaffold first:
```bash
./bin/travel scaffold-itinerary
```

### 3. Pre-check date range

Weather API supports 16-day forecast. If trip is >16 days away, forecast may be unavailable.

### 4. Fetch weather

```bash
./bin/travel fetch-weather --dest <slug>
```

Adds `weather` field to each itinerary day (temp, feels_like, precipitation, weather_code). Data source: Open-Meteo API (free, no key required).

### 5. Verify

```bash
./bin/travel itinerary
```

Each day should show weather data with `feels_like_max`. If missing, retry or check `src/services/weather-service.ts`.

### 6. Publish dashboard (optional)

After weather is fetched, publish via `/stage4-publish-dashboard` when the
user explicitly asks to deploy or refresh the live dashboard.

## Error Handling

| Error | Cause | Fix |
|-------|-------|-----|
| Destination not found | Slug not in `destination_config` table | Check exact slug via `turso-exec "SELECT slug FROM destination_config"` |
| Itinerary not scaffolded | P5 status = pending | `./bin/travel scaffold-itinerary` |
| Dates outside window | Trip >16 days away | Wait until closer to departure |
| No feels_like data | API response changed | Check Open-Meteo status, retry |

## See Also

- `src/services/weather-service.ts` — Weather API implementation
- `/stage4-publish-dashboard` — Explicit dashboard publish and verification
- `/deploy-dashboard` — Lower-level Cloudflare deploy steps
- `/new-destination` — Add new destination to config
