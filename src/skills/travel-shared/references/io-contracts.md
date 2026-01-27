# IO Contracts (Shared)

These are the stable “envelope” contracts used by all travel skills.

## Common Input

```ts
interface TravelSkillCommonInput {
  active_destination: string; // slug in travel-plan.json destinations.*

  date_filters: {
    start_date: string; // ISO-8601 date
    end_date: string;   // ISO-8601 date
    flexible: boolean;  // if true, search ±3 days (skill-specific)
    preferred_dates?: string[];
    avoid_dates?: string[];
  };

  pax: number;

  budget: {
    total_cap: number | null;
    per_person_cap: number | null;
  };

  constraints: {
    avoid_red_eye: boolean;
    prefer_direct: boolean;
    require_breakfast: boolean;
  };
}
```

## Common Output

```ts
interface TravelSkillCommonOutput<TOffer> {
  offers: TOffer[];
  chosen_offer: TOffer | null;
  provenance: Array<{
    source_id: string;
    scraped_at: string; // ISO-8601 timestamp
    offers_found: number;
    errors?: string[];
  }>;
  warnings: string[];
}
```

## Write-back rules (minimum)

- Write only into the skill’s declared `write_to` path under `destinations.{active_destination}.*`.
- Update `destinations.{active_destination}.{process}.status` and `.updated_at`.
- Mark/clear `cascade_state.destinations.{active_destination}.{process}` dirty flags as appropriate.

