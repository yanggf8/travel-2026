# Skill: /p3p4-packages

Search and compare flight+hotel package deals from OTA sources.

## Overview

| Field | Value |
|-------|-------|
| **Skill ID** | `p3p4_packages` |
| **Version** | `1.0.0` |
| **Schema Compatibility** | `travel-plan.json ^4.2.0` |
| **Write Path** | `destinations.{active_destination}.process_3_4_packages.results` |
| **Triggers** | On select: `cascade_package_select` |

## Input Contract

```typescript
interface P3P4PackagesInput {
  // Required: which destination branch to search for
  active_destination: string;  // slug from destinations.*

  // Date parameters
  date_filters: {
    start_date: string;        // ISO-8601 date
    end_date: string;          // ISO-8601 date
    flexible: boolean;         // if true, search ±3 days
    preferred_dates?: string[];  // prioritize these dates
    avoid_dates?: string[];      // deprioritize/exclude
  };

  // Travelers
  pax: number;  // default: from budget.pax

  // Budget constraints
  budget: {
    total_cap: number | null;      // max total for all pax
    per_person_cap: number | null; // max per person
  };

  // Search preferences
  constraints: {
    avoid_red_eye: boolean;      // no flights departing 00:00-05:00
    prefer_direct: boolean;      // prefer non-stop flights
    require_breakfast: boolean;  // must include breakfast
  };

  // OTA sources to search (default: all supported)
  sources?: string[];  // source_id from ota_sources.*
}
```

## Output Contract

```typescript
interface P3P4PackagesOutput {
  // Normalized offers conforming to canonical_offer_schema
  offers: CanonicalOffer[];

  // User-selected offer (null until selection made)
  chosen_offer: CanonicalOffer | null;

  // Audit trail
  provenance: Array<{
    source_id: string;
    scraped_at: string;  // ISO-8601
    offers_found: number;
    errors?: string[];
  }>;

  // Issues encountered
  warnings: string[];
}
```

## Canonical Offer Schema

All scrapers MUST normalize to this shape:

```typescript
interface CanonicalOffer {
  // Identity (required)
  id: string;              // format: {source_id}_{product_code}
  source_id: string;       // from ota_sources.*.source_id
  product_code: string;    // OTA's product identifier
  url: string;             // booking URL
  scraped_at: string;      // ISO-8601 timestamp

  // Type (required)
  type: 'package' | 'flight' | 'hotel' | 'activity';
  duration_days: number;   // required for type=package

  // Pricing (required)
  currency: string;        // default: TWD
  price_per_person: number;
  price_total: number;     // computed: price_per_person * pax
  availability: 'available' | 'sold_out' | 'limited' | 'unknown';
  seats_remaining: number | null;

  // Flight details (required for package/flight)
  flight: {
    airline: string;
    airline_code: string;  // IATA 2-letter
    outbound: {
      flight_number: string;
      departure_airport_code: string;  // IATA 3-letter
      arrival_airport_code: string;
      departure_time: string;  // HH:MM or ISO-8601
      arrival_time: string;
    };
    return: { /* same structure */ } | null;
  };

  // Hotel details (required for package/hotel)
  hotel: {
    name: string;
    slug: string;          // snake_case identifier
    area: string;
    star_rating: number | null;
    access: string[];      // transit directions
  };

  // Inclusions
  includes: Array<'light_breakfast' | 'full_breakfast' | 'airport_transfer' | 'wifi' | 'luggage'>;

  // Date-specific pricing (for flexible date search)
  date_pricing: {
    [date: string]: {
      price: number;
      availability: string;
      seats_remaining: number | null;
    };
  };

  // Best value summary
  best_value: {
    date: string;
    price_per_person: number;
    price_total: number;
  };

  // Evaluation
  pros: string[];
  cons: string[];
}
```

## Behavior

### 1. Search Phase

```
For each source_id in sources (or all supported OTAs):
  1. Load scraper for source_id
  2. Construct search URL from:
     - destination: destinations.{active}.process_2_destination.primary_airport
     - dates: date_filters.*
     - pax: pax
  3. Execute scraper (respect rate_limit)
  4. Normalize results to CanonicalOffer[]
  5. Record provenance
```

### 2. Filter Phase

```
For each offer:
  1. If budget.per_person_cap: exclude if price_per_person > cap
  2. If budget.total_cap: exclude if price_total > cap
  3. If constraints.avoid_red_eye: exclude if departure 00:00-05:00
  4. If constraints.require_breakfast: exclude if !includes.breakfast
  5. If date in avoid_dates: mark as deprioritized
```

### 3. Rank Phase

```
Score each offer:
  +10 if date in preferred_dates
  +5 if availability == 'available'
  +3 if includes breakfast
  -5 if date in avoid_dates
  -10 if availability == 'sold_out'

Sort by: score DESC, price_per_person ASC
```

### 4. Write Phase

```
Write to: destinations.{active_destination}.process_3_4_packages.results
{
  offers: [sorted CanonicalOffer[]],
  chosen_offer: null,
  provenance: [{source_id, scraped_at, offers_found}],
  warnings: [string[]]
}

Update: cascade_state.destinations.{active}.process_3_4_packages
{
  dirty: false,
  last_changed: now()
}
```

### 5. Select Phase (User Action)

When user selects an offer:

```
1. Set results.chosen_offer = selected offer
2. Set selected_offer_id = offer.id
3. Trigger cascade_package_select:
   - Copy offer.flight → process_3_transportation.flight
   - Copy offer.hotel → process_4_accommodation.hotel
   - Set process_3_transportation.source = 'package'
   - Set process_4_accommodation.source = 'package'
4. Update status = 'selected'
```

## Scraper Interface

Each OTA scraper must implement:

```typescript
interface OTAScraper {
  source_id: string;

  // Build search URL
  buildUrl(params: {
    destination_airport: string;
    start_date: string;
    end_date: string;
    pax: number;
  }): string;

  // Execute scrape and return raw data
  scrape(url: string): Promise<RawScraperResult>;

  // Normalize to canonical shape
  normalize(raw: RawScraperResult, pax: number): CanonicalOffer[];
}
```

## Example Invocation

```
/p3p4-packages

Input (from travel-plan.json):
- active_destination: tokyo_2026
- dates: 2026-02-11 to 2026-02-15 (flexible)
- preferred_dates: [2026-02-21, 2026-02-22]
- avoid_dates: [2026-02-14, 2026-02-15, 2026-02-16]
- pax: 2
- constraints: {avoid_red_eye: true, require_breakfast: false}
- sources: [besttour]  # or omit for all supported

Output:
- offers: [1 package from besttour]
- best_value: 2026-02-22 @ TWD 18,388/person
- warnings: ["Feb 11-13 sold out", "Return flight unknown"]
```

## Error Handling

| Error | Action |
|-------|--------|
| Scraper timeout | Record in provenance.errors, continue with other sources |
| Rate limit hit | Wait and retry once, then skip source |
| Invalid response | Log warning, exclude malformed offers |
| No offers found | Return empty offers[], warning "No packages found" |

## Idempotency

- Re-running skill overwrites `results` completely
- Previous offers are not merged (fresh search each time)
- `selected_offer_id` is preserved unless explicitly cleared
- If selected offer no longer exists in new results, set `chosen_offer = null` and warn

## Dependencies

- Requires: `process_1_date_anchor.status == 'confirmed'`
- Requires: `destinations.{active}.process_2_destination.status == 'confirmed'`
- Blocks: None (parallel with p3_flights, p4_hotels)
- Triggers on select: `cascade_package_select`
