# Canonical Offer (Shared)

All OTA scrapers normalize results into a single canonical model so skills can compose.

Source of truth (data): `data/travel-plan.json` → `canonical_offer_schema`.
Zod schema: `src/state/schemas.ts` → `OfferSchema`.

## Minimal required fields

```ts
type Availability = 'available' | 'sold_out' | 'limited' | 'unknown';
type PackageSubtype = 'fit' | 'group' | 'semi_fit' | 'unknown';

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

  // Package-specific (optional additive fields)
  package_subtype?: PackageSubtype; // FIT vs group distinction
  guided?: boolean;                  // Has tour guide/leader
  meals_included?: number;           // Number of meals included
}
```

## Package Subtype Classification

| Subtype | Description | Keywords |
|---------|-------------|----------|
| `fit` | Free Independent Travel (機加酒) | 自由行, 機加酒, 自助 |
| `group` | Guided group tour (跟團) | 團體, 跟團, 領隊, 導遊 |
| `semi_fit` | Hybrid with free days (伴自由) | 半自由, 伴自由, 自由時間 |
| `unknown` | Cannot determine | - |

## Package extensions (common)

- `duration_days`
- `flight` (outbound/return segments)
- `hotel`
- `includes`
- `date_pricing` and `best_value`
- `package_subtype`, `guided`, `meals_included` (optional additive fields)
