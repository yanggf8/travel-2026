# Canonical Offer (Shared)

All OTA scrapers normalize results into a single canonical model so skills can compose.

Source of truth (data): `data/travel-plan.json` → `canonical_offer_schema`.

## Minimal required fields

```ts
type Availability = 'available' | 'sold_out' | 'limited' | 'unknown';

interface CanonicalOffer {
  id: string;        // {source_id}_{product_code}
  source_id: string; // ota_sources.*.source_id
  product_code: string;
  url: string;
  scraped_at: string; // ISO-8601 timestamp

  type: 'package' | 'flight' | 'hotel' | 'activity';
  currency: string; // default TWD for TW-market OTAs

  price_per_person: number;
  price_total: number; // typically derived: price_per_person * pax

  availability: Availability;
}
```

## Package extensions (common)

- `duration_days`
- `flight` (outbound/return segments)
- `hotel`
- `includes`
- `date_pricing` and `best_value`

