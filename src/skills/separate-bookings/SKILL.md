---
name: separate-bookings
description: Compare package tour vs separate flight+hotel booking to find the best value option.
version: 1.0.0
requires_skills: [travel-shared]
requires_processes: [process_1_date_anchor, process_2_destination]
provides_processes: []
---

# /separate-bookings

## Purpose

Compare the total cost of an OTA package tour against booking flights and hotels
individually. Accounts for leave days, exchange rates, and per-person pricing to
produce a side-by-side comparison.

## Shared references

- `../travel-shared/references/io-contracts.md`
- `../travel-shared/references/ota-registry.md`

## Agent-First Defaults

- Automatically gather flight prices (Trip.com) and hotel prices (Booking.com) for
  the same date range as scraped package offers.
- Use `src/utilities/holiday-calculator.ts` for leave day calculations (not manual
  weekday counting).
- Present results as a ranked comparison table; ask only when user preference
  changes the outcome (budget cap, hotel class, must-have inclusions).

## CLI Tools

| Command | Purpose |
|---------|---------|
| `npm run compare-trips -- --input <file>` | Compare package vs separate from JSON input |
| `npm run compare-dates -- --start <date> --end <date> --nights <n>` | FIT vs separate across date range |
| `npm run view:prices -- --flights <file> --hotel-per-night <n> --nights <n> --package <n>` | Package vs separate price matrix |

## Data Sources

### Flights (Trip.com)
- Scrape outbound and return as separate one-way searches (`flighttype=ow`)
- Prices in USD; convert to TWD using `src/config/constants.ts` exchange rate
- Use `scripts/scrape_date_range.py` for multi-date comparison

### Hotels (Booking.com)
- Use `zh-tw` locale, `selected_currency=TWD`
- Requires `dest_id` parameter (not city name)
- Use `scripts/scrape_package.py` for scraping

### Packages (OTA)
- Use scraped data from `/p3p4-packages` or `/scrape-ota`
- Compare against best separate booking combination

## Output

Comparison table with columns:
- Departure date, day of week
- Flight cost (outbound + return, TWD)
- Hotel cost (per-night * nights, TWD)
- Separate total (flight + hotel)
- Package price (from OTA)
- Difference (package - separate)
- Leave days needed (from holiday calculator)

## Holiday Awareness

Always use `calculateLeave()` from `src/utilities/holiday-calculator.ts` for
leave day calculations. This ensures:
- Taiwan public holidays are correctly identified
- Makeup workdays (補班) are counted as leave days
- Weekend days are not double-counted with holidays
