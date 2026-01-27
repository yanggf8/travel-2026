# Date Filters (Shared)

## Semantics

- `start_date` / `end_date` are inclusive travel window bounds for search.
- `flexible: true` means the skill may explore a small neighborhood around the window (default convention: ±3 days) while keeping a deterministic output order.
- `preferred_dates` increases rank/priority (never a hard constraint unless the skill explicitly says so).
- `avoid_dates` deprioritizes (or excludes) dates; the skill must document which.

## Determinism rules

- Always sort candidate dates lexicographically (ISO date strings) before searching.
- Always sort final offers deterministically (primary score desc, then price asc, then stable tie-break such as `id`).

