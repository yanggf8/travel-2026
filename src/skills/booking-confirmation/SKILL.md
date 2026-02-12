---
name: booking-confirmation
description: Guided workflow for post-booking data entry and verification to prevent wrong flight/hotel data
version: 1.0.0
requires_skills: [travel-shared]
requires_processes: [process_3_4_packages]
provides_processes: []
---

# /booking-confirmation

## Purpose

Prevent common post-booking data errors: wrong flight numbers, wrong hotel, stale pricing, missing booking references.

## When to Use

After package booking confirmed, separate flight+hotel bookings confirmed, or receiving confirmation emails.

## Workflow

### 1. Verify package selection

```bash
npm run view:status
```

Check: correct offer selected, date matches booking, price matches what you paid.

If wrong: use `select-offer` to fix before marking booked.

### 2. Verify flight details

```bash
npm run view:transport
```

Compare confirmation email against displayed flights. Common issues:
- Package shows "Tigerair" but actual booking is Scoot
- Flight times changed after booking
- Terminal changed (T1 to T2)

If flights are wrong, update the chosen offer's flight data via StateManager, then re-run `mark-booked`.

### 3. Verify hotel details

Check hotel name, check-in/check-out dates, and room type match confirmation.

### 4. Update actual price paid

```bash
npm run travel -- update-offer <offer-id> <date> available <actual-price> --source booking_confirmation
```

Offer prices are estimates; actual price may differ (discounts, surcharges, currency conversion).

### 5. Mark as booked

```bash
npm run travel -- mark-booked --dest <slug>
```

Sets package/flight/hotel status to `booked`, emits booking event.

### 6. Add booking references

```bash
npm run travel -- set-activity-booking 2 morning "teamLab Borderless" booked --ref "TLB-20260214-001"
```

### 7. Sync to Turso (optional)

```bash
npm run travel -- sync-bookings
```

## Verification Checklist

```bash
# Status shows "booked"
npm run view:status

# Flight details match confirmation email
npm run view:transport

# Booking references recorded
npm run travel -- query-bookings --dest <slug> --status booked

# No overdue book_by dates
npm run travel -- validate-itinerary --severity warning
```

## Common Mistakes

- Marking booked before verifying data (locks in wrong info)
- Skipping flight detail verification (wrong airline/times in itinerary)
- Not recording booking references (can't find confirmation at airport)
- Forgetting to update actual price (budget tracking shows wrong total)

## See Also

- `references/db-first-pattern.md` — Why to use CLI/StateManager
- `/separate-bookings` — For non-package bookings
- `/pre-trip-checklist` — Pre-departure verification
