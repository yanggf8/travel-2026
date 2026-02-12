---
name: pre-trip-checklist
description: Pre-departure verification checklist to catch overdue bookings and missing data
version: 1.0.0
requires_skills: [travel-shared]
requires_processes: [process_3_transportation, process_4_accommodation, process_5_daily_itinerary]
provides_processes: []
---

# /pre-trip-checklist

## Purpose

Catch overdue bookings, missing data, and unfinished tasks before departure.

## When to Use

- 1 week before departure (catch overdue bookings)
- 1 day before departure (final verification)

## Workflow

### 1. Check booking deadlines

```bash
npm run travel -- validate-itinerary --severity warning
```

Look for activities with `book_by` dates in the past and status still `pending`.

### 2. Check pending bookings

```bash
npm run travel -- query-bookings --dest <slug> --status pending
```

Package, flight, and hotel should all be `booked` (not `selected` or `booking`).

### 3. Check booking references

```bash
npm run travel -- query-bookings --dest <slug> --category activity --status booked
```

Verify all booked activities have confirmation numbers recorded.

### 4. Check weather data

```bash
npm run view:itinerary
```

Each day should have a `weather` object. If missing:
```bash
npm run travel -- fetch-weather --dest <slug>
```

### 5. Check dashboard deployment

```bash
curl "https://trip-dashboard.<subdomain>.workers.dev/?plan=<slug>"
```

Should return HTML dashboard. If 404 or error, run `/deploy-dashboard`.

### 6. Verify transport details

```bash
npm run view:transport
```

Check flight times match confirmation email, airport transfer plan set.

## Checklist

```
[ ] All bookings confirmed (no pending status)
[ ] All booking references recorded
[ ] No overdue book_by dates
[ ] Weather data fetched for all days
[ ] Dashboard deployed and accessible
[ ] Flight details verified against confirmation
[ ] Airport transfer plan set (arrival + departure)
[ ] Restaurant reservations made (if needed)
```

## See Also

- `/booking-confirmation` — Post-booking verification
- `/weather-update` — Fetch weather data
- `/deploy-dashboard` — Deploy trip dashboard
