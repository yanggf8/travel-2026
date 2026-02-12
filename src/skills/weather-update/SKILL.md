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
cat data/destinations.json | jq '.destinations | keys'
```

Common issue: trying to fetch weather for "kyoto" when only "osaka_kyoto_2026" exists.

### 2. Pre-check itinerary status

```bash
npm run view:status
```

P5 status must be `researching` or later (not `pending`). If pending, scaffold first:
```bash
npm run travel -- scaffold-itinerary
```

### 3. Pre-check date range

Weather API supports 16-day forecast. If trip is >16 days away, forecast may be unavailable.

### 4. Fetch weather

```bash
npm run travel -- fetch-weather --dest <slug>
```

Adds `weather` field to each itinerary day (temp, feels_like, precipitation, weather_code). Data source: Open-Meteo API (free, no key required).

### 5. Verify

```bash
npm run view:itinerary
```

Each day should show weather data with `feels_like_max`. If missing, retry or check `src/services/weather-service.ts`.

### 6. Deploy dashboard (optional)

After weather fetched, deploy via `/deploy-dashboard` skill.

## Error Handling

| Error | Cause | Fix |
|-------|-------|-----|
| Destination not found | Slug not in destinations.json | Check exact slug spelling |
| Itinerary not scaffolded | P5 status = pending | `npm run travel -- scaffold-itinerary` |
| Dates outside window | Trip >16 days away | Wait until closer to departure |
| No feels_like data | API response changed | Check Open-Meteo status, retry |

## See Also

- `src/services/weather-service.ts` — Weather API implementation
- `/deploy-dashboard` — Deploy trip dashboard with weather
- `/new-destination` — Add new destination to config
