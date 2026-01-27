# OTA Registry (Shared)

Source of truth (data): `data/travel-plan.json` → `ota_sources`.

## Expected fields

```ts
interface OtaSourceRegistryEntry {
  source_id: string;
  display_name: string;
  type: Array<'package' | 'flight' | 'hotel' | 'activity'>;
  base_url: string;
  markets: string[];   // e.g. ["TW"]
  currency: string;    // e.g. "TWD"
  rate_limit: { requests_per_minute: number };
  auth_required: boolean;
  supported: boolean;
}
```

## Normalization expectations

- Each scraper maps raw offers to `CanonicalOffer` consistently.
- `id` stays stable across runs for the same `product_code`.
- Capture scrape metadata in `provenance[]` even on partial failure.

