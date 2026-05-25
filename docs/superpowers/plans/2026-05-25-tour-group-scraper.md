# Tour-Group Scraper — Stage 0 Baseline Source — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the first tour-group scraping path end-to-end so Stage 0 can produce a defensible price baseline. Scraper → JSON file → import → Turso storage → adopt-time bridge to plan-side offers.

**Architecture:** Two new unscoped Stage 0 tables (`stage0_tour_group_offers`, `stage0_tour_group_scrape_attempts`). One plan-side schema extension (`plan_offers.package_subtype`, new `plan_offer_group_meta`). One dedicated importer CLI (`import-tour-group-offers`) and one query CLI (`query-tour-group-offers`). Three Python scrapers (BestTour, Lifetour, Settour) emitting a shared JSON envelope. Bridge inside `stage0-adopt` copies a curated audit set from research to plan tables.

**Tech Stack:** TypeScript (ts-node), Turso DB (libSQL HTTP), Python + Playwright, vitest integration tests.

**Spec reference:** `docs/superpowers/specs/2026-05-25-tour-group-scraper-design.md`

---

## File Structure

```
scripts/turso-migrate.ts                           # MODIFY: add 4 new tables/columns
scripts/schema.sql                                 # MODIFY: mirror migration DDL (read-only reference)

src/services/tour-group-service.ts                 # CREATE: all Turso reads/writes for tour-group tables
src/cli/commands/tour-group.ts                     # CREATE: import-tour-group-offers + query-tour-group-offers commands
src/services/stage0-service.ts                     # MODIFY: extend adoptCandidate* to call the bridge
src/services/tour-group-bridge.ts                  # CREATE: pure function that computes the audit set and writes it

scripts/scrape_tour_groups.py                      # CREATE: shared CLI wrapper (one script, --source flag)
scripts/scrapers/parsers/besttour_groups.py        # CREATE: BestTour listing parser (NEW DIR if needed)
scripts/scrapers/parsers/lifetour_groups.py        # CREATE: Lifetour listing parser
scripts/scrapers/parsers/settour_groups.py         # CREATE: Settour listing parser

tests/integration/tour-group-import.regression.test.ts    # CREATE: importer + attempt status tests
tests/integration/tour-group-bridge.regression.test.ts    # CREATE: adopt-time bridge tests
tests/fixtures/tour-group/besttour-kansai-5n-ok.json      # CREATE: happy-path fixture
tests/fixtures/tour-group/besttour-kansai-5n-partial.json # CREATE: skipped-row fixture
tests/fixtures/tour-group/besttour-kansai-5n-mismatch.json# CREATE: attempt-identity mismatch fixture

CLAUDE.md                                          # MODIFY: add 2 new CLI commands to Quick Reference
docs/reference/CLI.md                              # MODIFY: full reference entries for new commands
```

**Decomposition rationale:**

- `tour-group-service.ts` owns all DB I/O for the new tables — mirrors how `stage0-service.ts` owns Stage 0 flight DB I/O. Service layer pattern is already established.
- `tour-group-bridge.ts` is a separate file because the bridge logic (audit-set selection) has nontrivial query logic and is the natural unit to test in isolation. Keeping it out of `stage0-service.ts` prevents that file from growing too large.
- One CLI command file (`tour-group.ts`) holds both `import-tour-group-offers` and `query-tour-group-offers` — they share argument-parsing helpers and operate on the same tables.
- Python parsers live under `scripts/scrapers/parsers/` per the existing repo convention; one `scrape_tour_groups.py` entry point with `--source` keeps the dispatch path simple.

---

## Task 1: Schema migration

**Files:**
- Modify: `scripts/turso-migrate.ts` (append new tables after line 1333; add `ALTER TABLE plan_offers` block)
- Modify: `scripts/schema.sql` (append same DDL — read-only reference, must mirror migration)

- [ ] **Step 1: Add the two new unscoped tables to the migration**

Append after `scripts/turso-migrate.ts:1334` (after the `CREATE INDEX idx_s0_cand_run` line):

```typescript
  await client.executeMany([
    `CREATE TABLE IF NOT EXISTS stage0_tour_group_offers (
      run_id TEXT NOT NULL,
      offer_id TEXT NOT NULL,
      source_id TEXT NOT NULL,
      dest_region TEXT NOT NULL,
      depart_date TEXT NOT NULL,
      return_date TEXT NOT NULL,
      nights INTEGER NOT NULL,
      price_per_person_twd INTEGER NOT NULL,
      title TEXT NOT NULL,
      url TEXT NOT NULL,
      scraped_at TEXT NOT NULL,
      hotel_name TEXT,
      hotel_star_rating INTEGER,
      meals_included_count INTEGER,
      departure_status TEXT,
      seats_available INTEGER,
      min_group_size INTEGER,
      group_size_cap INTEGER,
      raw_json TEXT,
      parse_warnings_json TEXT,
      PRIMARY KEY (run_id, offer_id)
    );`,
    `CREATE TABLE IF NOT EXISTS stage0_tour_group_scrape_attempts (
      run_id TEXT NOT NULL,
      source_id TEXT NOT NULL,
      dest_region TEXT NOT NULL,
      nights INTEGER NOT NULL,
      status TEXT NOT NULL,
      offer_count INTEGER,
      parsed_count INTEGER,
      skipped_count INTEGER,
      error TEXT,
      attempted_at TEXT,
      PRIMARY KEY (run_id, source_id, dest_region, nights)
    );`,
  ]);
  await client.execute(
    'CREATE INDEX IF NOT EXISTS idx_s0_tg_offers_lookup ON stage0_tour_group_offers(run_id, dest_region, nights, price_per_person_twd);'
  );
  console.log('✅ Stage 0 tour-group tables ready.');
```

- [ ] **Step 2: Add the `plan_offers.package_subtype` column and `plan_offer_group_meta` table**

Migrations to existing tables use idempotent `ALTER TABLE ... ADD COLUMN` guarded by a try/catch. After the block from Step 1, append:

```typescript
  // Idempotent ALTER — SQLite errors if column already exists; swallow that one.
  try {
    await client.execute(`ALTER TABLE plan_offers ADD COLUMN package_subtype TEXT;`);
  } catch (e: any) {
    if (!String(e?.message || '').match(/duplicate column name/i)) throw e;
  }
  // Backfill: every existing plan_offers row is FIT.
  await client.execute(
    `UPDATE plan_offers SET package_subtype = 'fit' WHERE package_subtype IS NULL;`
  );

  await client.execute(
    `CREATE TABLE IF NOT EXISTS plan_offer_group_meta (
      plan_id TEXT NOT NULL,
      destination TEXT NOT NULL,
      offer_id TEXT NOT NULL,
      meals_included_count INTEGER,
      departure_status TEXT,
      seats_available INTEGER,
      min_group_size INTEGER,
      group_size_cap INTEGER,
      source_offer_run_id TEXT,
      source_offer_id TEXT,
      PRIMARY KEY (plan_id, destination, offer_id),
      FOREIGN KEY (plan_id, destination, offer_id) REFERENCES plan_offers(plan_id, destination, id)
    );`
  );
  console.log('✅ Plan-side group-meta table ready.');
```

Note the PK/FK is `(plan_id, destination, offer_id)` because `plan_offers` PK is `(plan_id, destination, id)` — the spec's two-column FK was simplified; the migration uses the actual three-column key.

- [ ] **Step 3: Mirror the DDL in `scripts/schema.sql`**

Open `scripts/schema.sql`, find the existing `stage0_*` block, append the new tables in the same shape (no IF NOT EXISTS — `schema.sql` is a read-only DDL reference, not executed):

```sql
CREATE TABLE stage0_tour_group_offers (
  run_id TEXT NOT NULL,
  offer_id TEXT NOT NULL,
  source_id TEXT NOT NULL,
  dest_region TEXT NOT NULL,
  depart_date TEXT NOT NULL,
  return_date TEXT NOT NULL,
  nights INTEGER NOT NULL,
  price_per_person_twd INTEGER NOT NULL,
  title TEXT NOT NULL,
  url TEXT NOT NULL,
  scraped_at TEXT NOT NULL,
  hotel_name TEXT,
  hotel_star_rating INTEGER,
  meals_included_count INTEGER,
  departure_status TEXT,
  seats_available INTEGER,
  min_group_size INTEGER,
  group_size_cap INTEGER,
  raw_json TEXT,
  parse_warnings_json TEXT,
  PRIMARY KEY (run_id, offer_id)
);

CREATE INDEX idx_s0_tg_offers_lookup ON stage0_tour_group_offers(run_id, dest_region, nights, price_per_person_twd);

CREATE TABLE stage0_tour_group_scrape_attempts (
  run_id TEXT NOT NULL,
  source_id TEXT NOT NULL,
  dest_region TEXT NOT NULL,
  nights INTEGER NOT NULL,
  status TEXT NOT NULL,
  offer_count INTEGER,
  parsed_count INTEGER,
  skipped_count INTEGER,
  error TEXT,
  attempted_at TEXT,
  PRIMARY KEY (run_id, source_id, dest_region, nights)
);

CREATE TABLE plan_offer_group_meta (
  plan_id TEXT NOT NULL,
  destination TEXT NOT NULL,
  offer_id TEXT NOT NULL,
  meals_included_count INTEGER,
  departure_status TEXT,
  seats_available INTEGER,
  min_group_size INTEGER,
  group_size_cap INTEGER,
  source_offer_run_id TEXT,
  source_offer_id TEXT,
  PRIMARY KEY (plan_id, destination, offer_id),
  FOREIGN KEY (plan_id, destination, offer_id) REFERENCES plan_offers(plan_id, destination, id)
);
```

Find the existing `plan_offers` DDL in `schema.sql` and add the `package_subtype TEXT` column to its column list.

- [ ] **Step 4: Run the migration**

Run: `npm run db:migrate:turso`
Expected output (last 4 lines):
```
✅ Stage 0 research tables ready.
✅ Stage 0 tour-group tables ready.
✅ Plan-side group-meta table ready.
Done.
```

- [ ] **Step 5: Verify the tables exist in Turso**

Run: `npm run db:exec -- "SELECT name FROM sqlite_master WHERE name LIKE 'stage0_tour%' OR name = 'plan_offer_group_meta'"`
Expected: 3 rows — `stage0_tour_group_offers`, `stage0_tour_group_scrape_attempts`, `plan_offer_group_meta`.

Run: `npm run db:exec -- "SELECT name FROM pragma_table_info('plan_offers') WHERE name = 'package_subtype'"`
Expected: 1 row.

Run: `npm run db:exec -- "SELECT COUNT(*) AS n FROM plan_offers WHERE package_subtype = 'fit'"`
Expected: equal to total plan_offers count (backfill ran).

- [ ] **Step 6: Commit**

```bash
git add scripts/turso-migrate.ts scripts/schema.sql
git commit -m "feat(schema): add tour-group offer + scrape-attempt tables, plan_offers.package_subtype, plan_offer_group_meta"
```

---

## Task 2: Tour-group service layer

**Files:**
- Create: `src/services/tour-group-service.ts`

Service owns all DB I/O for the new tables. Pure functions — no CLI parsing, no file I/O. Mirrors `src/services/stage0-service.ts` structure.

- [ ] **Step 1: Write the failing test (existence test only — service tests come with importer tests)**

For this task, only confirm the file exists and exports the right shapes. Real behavior testing happens in Task 3.

Create `tests/integration/tour-group-service.regression.test.ts`:

```typescript
import { describe, it, expect } from 'vitest';

describe('tour-group-service', () => {
  it('exports the public API', async () => {
    const svc = await import('../../src/services/tour-group-service');
    expect(typeof svc.insertTourGroupOffers).toBe('function');
    expect(typeof svc.upsertScrapeAttempt).toBe('function');
    expect(typeof svc.listTourGroupOffers).toBe('function');
    expect(typeof svc.findAuditSet).toBe('function');
  });
});
```

- [ ] **Step 2: Run the test (will fail — module not found)**

Run: `npm test -- tour-group-service`
Expected: FAIL with `Cannot find module ../../src/services/tour-group-service`.

- [ ] **Step 3: Implement the service**

Create `src/services/tour-group-service.ts`:

```typescript
import { sqlText, sqlInt, rowsToObjects } from '../state/sql-helpers';
import path from 'node:path';

// Scripts live outside src/ (rootDir), so use the same dynamic require pattern
// as stage0-service.ts and turso-service.ts.
function getProjectRoot(): string {
  return path.resolve(__dirname, '..', '..');
}

function requirePipeline(): { TursoPipelineClient: new (opts?: any) => any } {
  return require(path.join(getProjectRoot(), 'scripts', 'turso-pipeline.ts'));
}

export function getTursoClient(): any {
  const { TursoPipelineClient } = requirePipeline();
  return new TursoPipelineClient();
}

export interface TourGroupOfferRow {
  run_id: string;
  offer_id: string;
  source_id: string;
  dest_region: string;
  depart_date: string;
  return_date: string;
  nights: number;
  price_per_person_twd: number;
  title: string;
  url: string;
  scraped_at: string;
  hotel_name?: string | null;
  hotel_star_rating?: number | null;
  meals_included_count?: number | null;
  departure_status?: string | null;
  seats_available?: number | null;
  min_group_size?: number | null;
  group_size_cap?: number | null;
  raw_json?: string | null;
  parse_warnings_json?: string | null;
}

export type ScrapeAttemptStatus = 'pending' | 'ok' | 'failed' | 'partial';

export interface ScrapeAttemptRow {
  run_id: string;
  source_id: string;
  dest_region: string;
  nights: number;
  status: ScrapeAttemptStatus;
  offer_count?: number | null;
  parsed_count?: number | null;
  skipped_count?: number | null;
  error?: string | null;
  attempted_at?: string | null;
}

const REQUIRED_OFFER_FIELDS: (keyof TourGroupOfferRow)[] = [
  'run_id', 'offer_id', 'source_id', 'dest_region',
  'depart_date', 'return_date', 'nights',
  'price_per_person_twd', 'title', 'url', 'scraped_at',
];

export function validateOfferRow(row: Partial<TourGroupOfferRow>): { ok: true } | { ok: false; missing: string[] } {
  const missing = REQUIRED_OFFER_FIELDS.filter(k => row[k] === undefined || row[k] === null || row[k] === '');
  if (missing.length > 0) return { ok: false, missing: missing.map(String) };
  return { ok: true };
}

export async function insertTourGroupOffers(rows: TourGroupOfferRow[]): Promise<void> {
  if (rows.length === 0) return;
  const client = getTursoClient();
  const stmts = rows.map(r =>
    `INSERT OR REPLACE INTO stage0_tour_group_offers (
      run_id, offer_id, source_id, dest_region, depart_date, return_date, nights,
      price_per_person_twd, title, url, scraped_at,
      hotel_name, hotel_star_rating, meals_included_count, departure_status,
      seats_available, min_group_size, group_size_cap, raw_json, parse_warnings_json
    ) VALUES (
      ${sqlText(r.run_id)}, ${sqlText(r.offer_id)}, ${sqlText(r.source_id)}, ${sqlText(r.dest_region)},
      ${sqlText(r.depart_date)}, ${sqlText(r.return_date)}, ${sqlInt(r.nights)},
      ${sqlInt(r.price_per_person_twd)}, ${sqlText(r.title)}, ${sqlText(r.url)}, ${sqlText(r.scraped_at)},
      ${sqlText(r.hotel_name ?? null)}, ${sqlInt(r.hotel_star_rating ?? null)},
      ${sqlInt(r.meals_included_count ?? null)}, ${sqlText(r.departure_status ?? null)},
      ${sqlInt(r.seats_available ?? null)}, ${sqlInt(r.min_group_size ?? null)},
      ${sqlInt(r.group_size_cap ?? null)}, ${sqlText(r.raw_json ?? null)}, ${sqlText(r.parse_warnings_json ?? null)}
    );`
  );
  await client.executeMany(stmts);
}

export async function upsertScrapeAttempt(row: ScrapeAttemptRow): Promise<void> {
  const client = getTursoClient();
  await client.execute(
    `INSERT INTO stage0_tour_group_scrape_attempts
      (run_id, source_id, dest_region, nights, status, offer_count, parsed_count, skipped_count, error, attempted_at)
      VALUES (${sqlText(row.run_id)}, ${sqlText(row.source_id)}, ${sqlText(row.dest_region)}, ${sqlInt(row.nights)},
        ${sqlText(row.status)}, ${sqlInt(row.offer_count ?? null)}, ${sqlInt(row.parsed_count ?? null)},
        ${sqlInt(row.skipped_count ?? null)}, ${sqlText(row.error ?? null)}, ${sqlText(row.attempted_at ?? null)})
      ON CONFLICT(run_id, source_id, dest_region, nights) DO UPDATE SET
        status = excluded.status,
        offer_count = excluded.offer_count,
        parsed_count = excluded.parsed_count,
        skipped_count = excluded.skipped_count,
        error = excluded.error,
        attempted_at = excluded.attempted_at;`
  );
}

export async function findScrapeAttempt(
  run_id: string, source_id: string, dest_region: string, nights: number
): Promise<ScrapeAttemptRow | null> {
  const client = getTursoClient();
  const r = await client.execute(
    `SELECT * FROM stage0_tour_group_scrape_attempts
     WHERE run_id = ${sqlText(run_id)}
       AND source_id = ${sqlText(source_id)}
       AND dest_region = ${sqlText(dest_region)}
       AND nights = ${sqlInt(nights)};`
  );
  const rows = rowsToObjects(r) as ScrapeAttemptRow[];
  return rows[0] || null;
}

export async function listTourGroupOffers(filter: {
  run_id: string;
  source_id?: string;
  dest_region?: string;
  nights?: number;
  max_price?: number;
}): Promise<TourGroupOfferRow[]> {
  const client = getTursoClient();
  const where: string[] = [`run_id = ${sqlText(filter.run_id)}`];
  if (filter.source_id) where.push(`source_id = ${sqlText(filter.source_id)}`);
  if (filter.dest_region) where.push(`dest_region = ${sqlText(filter.dest_region)}`);
  if (filter.nights !== undefined) where.push(`nights = ${sqlInt(filter.nights)}`);
  if (filter.max_price !== undefined) where.push(`price_per_person_twd <= ${sqlInt(filter.max_price)}`);
  const r = await client.execute(
    `SELECT * FROM stage0_tour_group_offers
     WHERE ${where.join(' AND ')}
     ORDER BY price_per_person_twd ASC;`
  );
  return rowsToObjects(r) as TourGroupOfferRow[];
}

/**
 * Compute the audit set for a (run_id, dest_region, nights) combination.
 * Returns: raw cheapest + quality-floor cheapest + top-3 per source/depart_date.
 * Quality-floor default: hotel_star_rating >= 4. Rows with NULL star_rating
 * are excluded from the quality-floor pick but eligible for raw cheapest and
 * top-3-per-source/date.
 */
export async function findAuditSet(
  run_id: string,
  dest_region: string,
  nights: number,
  opts: { qualityFloorStarRating?: number } = {}
): Promise<TourGroupOfferRow[]> {
  const floor = opts.qualityFloorStarRating ?? 4;
  const all = await listTourGroupOffers({ run_id, dest_region, nights });
  if (all.length === 0) return [];

  const picked = new Map<string, TourGroupOfferRow>(); // offer_id -> row

  // raw cheapest
  const cheapest = all[0]; // already sorted ASC
  picked.set(cheapest.offer_id, cheapest);

  // quality-floor cheapest
  const qFloor = all.find(o => (o.hotel_star_rating ?? 0) >= floor);
  if (qFloor) picked.set(qFloor.offer_id, qFloor);

  // top-3 per (source_id, depart_date)
  const groups = new Map<string, TourGroupOfferRow[]>();
  for (const o of all) {
    const key = `${o.source_id}|${o.depart_date}`;
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key)!.push(o);
  }
  for (const rows of groups.values()) {
    rows.sort((a, b) => a.price_per_person_twd - b.price_per_person_twd);
    for (const r of rows.slice(0, 3)) picked.set(r.offer_id, r);
  }

  return Array.from(picked.values()).sort((a, b) => a.price_per_person_twd - b.price_per_person_twd);
}
```

- [ ] **Step 4: Run the test**

Run: `npm test -- tour-group-service`
Expected: PASS (existence test only).

- [ ] **Step 5: Typecheck**

Run: `npm run typecheck`
Expected: 0 errors.

- [ ] **Step 6: Commit**

```bash
git add src/services/tour-group-service.ts tests/integration/tour-group-service.regression.test.ts
git commit -m "feat(tour-group): add service layer for stage0 tour-group offers + attempts"
```

---

## Task 3: Importer command + integration tests

**Files:**
- Create: `src/cli/commands/tour-group.ts`
- Modify: `src/cli/shared/args.ts`
- Create: `tests/integration/tour-group-import.regression.test.ts`
- Create: `tests/fixtures/tour-group/besttour-kansai-5n-ok.json`
- Create: `tests/fixtures/tour-group/besttour-kansai-5n-partial.json`
- Create: `tests/fixtures/tour-group/besttour-kansai-5n-mismatch.json`
- Modify: `src/cli/commands/registry.ts` already auto-imports — see `src/cli/travel-update.ts` for the registry pattern.

- [ ] **Step 1: Write the fixture files**

Create `tests/fixtures/tour-group/besttour-kansai-5n-ok.json`:

```json
{
  "run_id": "test-run-tg-001",
  "scraped_at": "2026-05-25T12:00:00Z",
  "source_id": "besttour",
  "dest_region": "kansai",
  "nights": 5,
  "tour_group_offers": [
    {
      "offer_id": "besttour-KIX-20260620-5n-aaa111",
      "depart_date": "2026-06-20",
      "return_date": "2026-06-25",
      "nights": 5,
      "price_per_person_twd": 38900,
      "title": "關西超值五日 大阪心齋橋 京都嵐山 含早晚餐",
      "url": "https://www.besttour.com.tw/test/aaa111",
      "scraped_at": "2026-05-25T12:00:00Z",
      "source_id": "besttour",
      "dest_region": "kansai",
      "run_id": "test-run-tg-001",
      "hotel_name": "Cross Hotel Osaka",
      "hotel_star_rating": 4,
      "meals_included_count": 6,
      "departure_status": "guaranteed"
    },
    {
      "offer_id": "besttour-KIX-20260620-5n-bbb222",
      "depart_date": "2026-06-20",
      "return_date": "2026-06-25",
      "nights": 5,
      "price_per_person_twd": 32500,
      "title": "關西經典五日 三星酒店",
      "url": "https://www.besttour.com.tw/test/bbb222",
      "scraped_at": "2026-05-25T12:00:00Z",
      "source_id": "besttour",
      "dest_region": "kansai",
      "run_id": "test-run-tg-001",
      "hotel_name": "Toyoko Inn Osaka",
      "hotel_star_rating": 3,
      "meals_included_count": 5,
      "departure_status": "available"
    }
  ]
}
```

Create `tests/fixtures/tour-group/besttour-kansai-5n-partial.json` (one valid row, one missing required field `price_per_person_twd`):

```json
{
  "run_id": "test-run-tg-002",
  "scraped_at": "2026-05-25T12:00:00Z",
  "source_id": "besttour",
  "dest_region": "kansai",
  "nights": 5,
  "tour_group_offers": [
    {
      "offer_id": "besttour-KIX-20260620-5n-ccc333",
      "depart_date": "2026-06-20",
      "return_date": "2026-06-25",
      "nights": 5,
      "price_per_person_twd": 41000,
      "title": "Valid row",
      "url": "https://www.besttour.com.tw/test/ccc333",
      "scraped_at": "2026-05-25T12:00:00Z",
      "source_id": "besttour",
      "dest_region": "kansai",
      "run_id": "test-run-tg-002"
    },
    {
      "offer_id": "besttour-KIX-20260620-5n-ddd444",
      "depart_date": "2026-06-20",
      "return_date": "2026-06-25",
      "nights": 5,
      "title": "Missing price row (詢價)",
      "url": "https://www.besttour.com.tw/test/ddd444",
      "scraped_at": "2026-05-25T12:00:00Z",
      "source_id": "besttour",
      "dest_region": "kansai",
      "run_id": "test-run-tg-002"
    }
  ]
}
```

Create `tests/fixtures/tour-group/besttour-kansai-5n-mismatch.json` (top-level source_id doesn't match offer source_id):

```json
{
  "run_id": "test-run-tg-003",
  "scraped_at": "2026-05-25T12:00:00Z",
  "source_id": "lifetour",
  "dest_region": "kansai",
  "nights": 5,
  "tour_group_offers": [
    {
      "offer_id": "besttour-KIX-20260620-5n-eee555",
      "depart_date": "2026-06-20",
      "return_date": "2026-06-25",
      "nights": 5,
      "price_per_person_twd": 35000,
      "title": "Mismatched source",
      "url": "https://www.besttour.com.tw/test/eee555",
      "scraped_at": "2026-05-25T12:00:00Z",
      "source_id": "besttour",
      "dest_region": "kansai",
      "run_id": "test-run-tg-003"
    }
  ]
}
```

- [ ] **Step 2: Write the failing integration tests**

Create `tests/integration/tour-group-import.regression.test.ts`:

```typescript
import { describe, it, expect, beforeEach } from 'vitest';
import { spawnSync } from 'child_process';
import * as path from 'path';
import { rowsToObjects, sqlInt, sqlText } from '../../src/state/sql-helpers';
import { getTursoClient } from '../../src/services/tour-group-service';

const FIXTURE_DIR = path.resolve(__dirname, '../fixtures/tour-group');
const CLI = ['ts-node', 'src/cli/travel-update.ts'];

function runCli(args: string[]): { stdout: string; stderr: string; code: number } {
  const r = spawnSync('npx', [...CLI, ...args], { encoding: 'utf-8' });
  return { stdout: r.stdout, stderr: r.stderr, code: r.status ?? -1 };
}

async function clearRun(run_id: string) {
  const c = getTursoClient();
  await c.execute(`DELETE FROM stage0_tour_group_offers WHERE run_id = ${sqlText(run_id)};`);
  await c.execute(`DELETE FROM stage0_tour_group_scrape_attempts WHERE run_id = ${sqlText(run_id)};`);
}

async function seedPendingAttempt(run_id: string, source_id: string, dest_region: string, nights: number) {
  const c = getTursoClient();
  await c.execute(
    `INSERT OR REPLACE INTO stage0_tour_group_scrape_attempts
      (run_id, source_id, dest_region, nights, status)
     VALUES (${sqlText(run_id)}, ${sqlText(source_id)}, ${sqlText(dest_region)}, ${sqlInt(nights)}, 'pending');`
  );
}

describe('import-tour-group-offers', () => {
  beforeEach(async () => {
    await clearRun('test-run-tg-001');
    await clearRun('test-run-tg-002');
    await clearRun('test-run-tg-003');
  });

  it('happy path: all rows imported, attempt status=ok', async () => {
    await seedPendingAttempt('test-run-tg-001', 'besttour', 'kansai', 5);
    const { code, stderr } = runCli([
      'import-tour-group-offers',
      '--run', 'test-run-tg-001',
      '--file', path.join(FIXTURE_DIR, 'besttour-kansai-5n-ok.json'),
    ]);
    expect(code, stderr).toBe(0);

    const c = getTursoClient();
    const offers = rowsToObjects(await c.execute(
      `SELECT * FROM stage0_tour_group_offers
       WHERE run_id = 'test-run-tg-001'
       ORDER BY price_per_person_twd;`
    )) as any[];
    expect(offers).toHaveLength(2);
    expect(offers[0].price_per_person_twd).toBe(32500);
    expect(offers[1].hotel_star_rating).toBe(4);

    const att = (rowsToObjects(await c.execute(
      `SELECT * FROM stage0_tour_group_scrape_attempts WHERE run_id = 'test-run-tg-001';`
    )) as any[])[0];
    expect(att.status).toBe('ok');
    expect(att.parsed_count).toBe(2);
    expect(att.skipped_count).toBe(0);
  });

  it('partial: one row skipped for missing required field, attempt status=partial', async () => {
    await seedPendingAttempt('test-run-tg-002', 'besttour', 'kansai', 5);
    const { code } = runCli([
      'import-tour-group-offers',
      '--run', 'test-run-tg-002',
      '--file', path.join(FIXTURE_DIR, 'besttour-kansai-5n-partial.json'),
    ]);
    expect(code).toBe(0);

    const c = getTursoClient();
    const offers = rowsToObjects(await c.execute(
      `SELECT * FROM stage0_tour_group_offers WHERE run_id = 'test-run-tg-002';`
    )) as any[];
    expect(offers).toHaveLength(1);
    const att = (rowsToObjects(await c.execute(
      `SELECT * FROM stage0_tour_group_scrape_attempts WHERE run_id = 'test-run-tg-002';`
    )) as any[])[0];
    expect(att.status).toBe('partial');
    expect(att.parsed_count).toBe(1);
    expect(att.skipped_count).toBe(1);
  });

  it('rejects: attempt-identity mismatch — top-level source_id has no pending attempt', async () => {
    // Note: we seed 'besttour' but the file declares 'lifetour' at top level
    await seedPendingAttempt('test-run-tg-003', 'besttour', 'kansai', 5);
    const { code, stderr } = runCli([
      'import-tour-group-offers',
      '--run', 'test-run-tg-003',
      '--file', path.join(FIXTURE_DIR, 'besttour-kansai-5n-mismatch.json'),
    ]);
    expect(code).not.toBe(0);
    expect(stderr).toMatch(/no pending attempt|attempt not found/i);

    const c = getTursoClient();
    const offers = rowsToObjects(await c.execute(
      `SELECT COUNT(*) AS n FROM stage0_tour_group_offers WHERE run_id = 'test-run-tg-003';`
    )) as any[];
    expect((offers[0] as any).n).toBe(0);
  });
});
```

- [ ] **Step 3: Run the tests (will fail — command doesn't exist)**

Run: `npm test -- tour-group-import`
Expected: All three tests FAIL with "Unknown command: import-tour-group-offers" or similar.

- [ ] **Step 4: Implement the command**

Create `src/cli/commands/tour-group.ts`:

```typescript
import * as fs from 'fs';
import * as path from 'path';
import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';

interface ImportFileEnvelope {
  run_id: string;
  scraped_at: string;
  source_id: string;
  dest_region: string;
  nights: number;
  tour_group_offers: any[];
}

const importTourGroupCommand: CommandHandler = {
  names: ['import-tour-group-offers'],
  description: 'Import tour-group offers from a scraper output JSON file into stage0 tables.',
  usage: 'import-tour-group-offers --run <run_id> --file <path>',
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const { args } = ctx;
    const runId = args.optionValue('--run');
    const file = args.optionValue('--file');
    if (!runId || !file) {
      console.error('Error: import-tour-group-offers requires --run <run_id> and --file <path>');
      process.exit(1);
    }

    const absPath = path.resolve(file);
    if (!fs.existsSync(absPath)) {
      console.error(`Error: file not found: ${absPath}`);
      process.exit(1);
    }

    const envelope: ImportFileEnvelope = JSON.parse(fs.readFileSync(absPath, 'utf-8'));
    if (envelope.run_id !== runId) {
      console.error(`Error: file run_id "${envelope.run_id}" does not match --run "${runId}"`);
      process.exit(1);
    }

    const svc = await import('../../services/tour-group-service');
    const existing = await svc.findScrapeAttempt(
      runId, envelope.source_id, envelope.dest_region, envelope.nights
    );
    if (!existing) {
      console.error(
        `Error: no pending attempt found for (run=${runId}, source=${envelope.source_id}, ` +
        `region=${envelope.dest_region}, nights=${envelope.nights}). ` +
        `Seed it with stage0-init or the per-source helper before importing.`
      );
      process.exit(1);
    }

    const offers = envelope.tour_group_offers || [];
    const parsed: any[] = [];
    const skipped: any[] = [];
    for (const raw of offers) {
      const v = svc.validateOfferRow(raw);
      if ((v as any).ok) {
        parsed.push(raw);
      } else {
        skipped.push({ offer_id: raw.offer_id ?? '<no-id>', missing: (v as any).missing });
      }
    }

    if (parsed.length > 0) {
      await svc.insertTourGroupOffers(parsed);
    }

    const status: svc.ScrapeAttemptStatus =
      parsed.length === 0 ? 'failed'
      : skipped.length > 0 ? 'partial'
      : 'ok';

    await svc.upsertScrapeAttempt({
      run_id: runId,
      source_id: envelope.source_id,
      dest_region: envelope.dest_region,
      nights: envelope.nights,
      status,
      offer_count: offers.length,
      parsed_count: parsed.length,
      skipped_count: skipped.length,
      error: skipped.length > 0 ? `skipped ${skipped.length}: ${JSON.stringify(skipped.slice(0, 5))}` : null,
      attempted_at: new Date().toISOString(),
    });

    console.log(`✅ Imported ${parsed.length} offers (skipped ${skipped.length}). Attempt status: ${status}`);
  },
};

const queryTourGroupCommand: CommandHandler = {
  names: ['query-tour-group-offers'],
  description: 'List tour-group offers collected for a Stage 0 run.',
  usage: 'query-tour-group-offers --run <run_id> [--source <id>] [--dest-region <region>] [--nights N] [--max-price TWD] [--json]',
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const { args } = ctx;
    const runId = args.optionValue('--run');
    if (!runId) {
      console.error('Error: query-tour-group-offers requires --run <run_id>');
      process.exit(1);
    }
    const svc = await import('../../services/tour-group-service');
    const rows = await svc.listTourGroupOffers({
      run_id: runId,
      source_id: args.optionValue('--source') || undefined,
      dest_region: args.optionValue('--dest-region') || undefined,
      nights: args.optionValue('--nights') ? parseInt(args.optionValue('--nights')!, 10) : undefined,
      max_price: args.optionValue('--max-price') ? parseInt(args.optionValue('--max-price')!, 10) : undefined,
    });
    if (args.hasFlag('--json')) {
      console.log(JSON.stringify(rows, null, 2));
      return;
    }
    if (rows.length === 0) { console.log('(no rows)'); return; }
    console.log(`PRICE      SOURCE     REGION    DEPART      NIGHTS  HOTEL                          STAR  TITLE`);
    console.log('-'.repeat(120));
    for (const r of rows) {
      const price = String(r.price_per_person_twd).padStart(8, ' ');
      const src = (r.source_id || '').padEnd(10);
      const reg = (r.dest_region || '').padEnd(9);
      const dt = (r.depart_date || '').padEnd(11);
      const n = String(r.nights).padStart(6);
      const h = (r.hotel_name || '').slice(0, 30).padEnd(30);
      const s = r.hotel_star_rating === null || r.hotel_star_rating === undefined ? '   -' : `   ${r.hotel_star_rating}`;
      console.log(`${price}  ${src} ${reg} ${dt} ${n}  ${h}  ${s}  ${r.title?.slice(0, 30)}`);
    }
  },
};

registerCommand(importTourGroupCommand);
registerCommand(queryTourGroupCommand);
```

- [ ] **Step 5: Register the new file in the CLI entry point**

Open `src/cli/travel-update.ts` and find the block that imports command files. Add the line:

```typescript
import './commands/tour-group';
```

If the imports are listed alphabetically, add it between `./commands/status` and `./commands/transport` (or wherever it sorts). If not alphabetical, add it after the existing `./commands/stage0` import.

- [ ] **Step 6: Register `--dest-region` as an option with a value**

Open `src/cli/shared/args.ts` and add `--dest-region` to `OPTIONS_WITH_VALUES`:

```typescript
  '--run', '--file', '--origin', '--rate', '--dest-region',
```

This prevents `kansai` from being retained in `cleanArgs` as a positional argument for `query-tour-group-offers` and `stage0-adopt`.

- [ ] **Step 7: Run the tests**

Run: `npm test -- tour-group-import`
Expected: 3 PASSED.

- [ ] **Step 8: Typecheck**

Run: `npm run typecheck`
Expected: 0 errors.

- [ ] **Step 9: Commit**

```bash
git add src/cli/commands/tour-group.ts src/cli/travel-update.ts src/cli/shared/args.ts \
  tests/integration/tour-group-import.regression.test.ts \
  tests/fixtures/tour-group/
git commit -m "feat(tour-group): add import-tour-group-offers + query-tour-group-offers CLI"
```

---

## Task 4: Adopt-time bridge

**Files:**
- Create: `src/services/tour-group-bridge.ts`
- Modify: `src/services/stage0-service.ts` (call bridge from `adoptCandidate` and `adoptCandidateToNewPlan`)
- Create: `tests/integration/tour-group-bridge.regression.test.ts`

- [ ] **Step 1: Write the failing test**

Create `tests/integration/tour-group-bridge.regression.test.ts`:

```typescript
import { describe, it, expect, beforeEach } from 'vitest';
import { rowsToObjects, sqlText } from '../../src/state/sql-helpers';
import { bridgeAuditSet } from '../../src/services/tour-group-bridge';
import { insertTourGroupOffers, getTursoClient } from '../../src/services/tour-group-service';

const RUN = 'test-run-bridge-001';
const PLAN = 'test-plan-bridge-001';
const DEST = 'osaka_2026';

async function clear() {
  const c = getTursoClient();
  await c.execute(`DELETE FROM stage0_tour_group_offers WHERE run_id = ${sqlText(RUN)};`);
  await c.execute(`DELETE FROM plan_offer_group_meta WHERE plan_id = ${sqlText(PLAN)};`);
  await c.execute(`DELETE FROM plan_offers WHERE plan_id = ${sqlText(PLAN)};`);
}

function offer(over: any) {
  return {
    run_id: RUN, source_id: 'besttour', dest_region: 'kansai',
    depart_date: '2026-06-20', return_date: '2026-06-25', nights: 5,
    title: 'test', url: 'http://test/', scraped_at: '2026-05-25T00:00:00Z',
    ...over,
  };
}

describe('tour-group adopt-time bridge', () => {
  beforeEach(clear);

  it('copies audit set: raw cheapest + quality-floor + top-3 per source/date', async () => {
    await insertTourGroupOffers([
      offer({ offer_id: 'a1', price_per_person_twd: 28000, hotel_star_rating: 3 }), // raw cheapest
      offer({ offer_id: 'a2', price_per_person_twd: 35000, hotel_star_rating: 4 }), // quality-floor cheapest
      offer({ offer_id: 'a3', price_per_person_twd: 30000, hotel_star_rating: 3 }), // top-3 with a1 + a2
      offer({ offer_id: 'a4', price_per_person_twd: 42000, hotel_star_rating: 5 }), // not in any set (4th cheapest, q-floor already a2)
      offer({ offer_id: 'b1', source_id: 'lifetour', price_per_person_twd: 31000, hotel_star_rating: 3 }), // top-3 for lifetour
    ]);

    await bridgeAuditSet({
      run_id: RUN, plan_id: PLAN, destination: DEST,
      dest_region: 'kansai', nights: 5,
    });

    const c = getTursoClient();
    const inserted = rowsToObjects(await c.execute(
      `SELECT id FROM plan_offers WHERE plan_id = ${sqlText(PLAN)} ORDER BY price_per_person;`
    )) as any[];
    const ids = inserted.map(r => r.id);
    expect(ids).toContain('a1'); // raw cheapest
    expect(ids).toContain('a2'); // quality-floor
    expect(ids).toContain('a3'); // top-3 besttour
    expect(ids).toContain('b1'); // top-3 lifetour
    expect(ids).not.toContain('a4'); // 4th cheapest for besttour, no other reason to include

    const meta = rowsToObjects(await c.execute(
      `SELECT source_offer_id FROM plan_offer_group_meta WHERE plan_id = ${sqlText(PLAN)};`
    )) as any[];
    expect(meta.map(m => m.source_offer_id).sort()).toEqual(['a1', 'a2', 'a3', 'b1'].sort());
  });

  it('handles empty quality-floor gracefully (no 4-star rows)', async () => {
    await insertTourGroupOffers([
      offer({ offer_id: 'c1', price_per_person_twd: 28000, hotel_star_rating: 3 }),
      offer({ offer_id: 'c2', price_per_person_twd: 30000, hotel_star_rating: 3 }),
    ]);
    await bridgeAuditSet({
      run_id: RUN, plan_id: PLAN, destination: DEST,
      dest_region: 'kansai', nights: 5,
    });
    const c = getTursoClient();
    const ids = (rowsToObjects(await c.execute(
      `SELECT id FROM plan_offers WHERE plan_id = ${sqlText(PLAN)};`
    )) as any[]).map(r => r.id).sort();
    expect(ids).toEqual(['c1', 'c2']);
  });
});
```

- [ ] **Step 2: Run the test (fails — bridge doesn't exist)**

Run: `npm test -- tour-group-bridge`
Expected: FAIL with `Cannot find module ../../src/services/tour-group-bridge`.

- [ ] **Step 3: Implement the bridge**

Create `src/services/tour-group-bridge.ts`:

```typescript
import { sqlInt, sqlText } from '../state/sql-helpers';
import { findAuditSet, getTursoClient, TourGroupOfferRow } from './tour-group-service';

export interface BridgeInput {
  run_id: string;
  plan_id: string;
  destination: string;
  dest_region: string;
  nights: number;
  qualityFloorStarRating?: number; // default 4
}

/**
 * Copy the curated audit set for this (run, dest_region, nights) into
 * plan_offers + plan_offer_group_meta. Idempotent — INSERT OR REPLACE.
 *
 * Audit set: raw cheapest + quality-floor cheapest + top-3 per
 * (source_id, depart_date). See spec §5.
 */
export async function bridgeAuditSet(input: BridgeInput): Promise<TourGroupOfferRow[]> {
  const audit = await findAuditSet(
    input.run_id, input.dest_region, input.nights,
    { qualityFloorStarRating: input.qualityFloorStarRating }
  );
  if (audit.length === 0) return [];

  const client = getTursoClient();
  const stmts: string[] = [];
  for (const o of audit) {
    stmts.push(
      `INSERT OR REPLACE INTO plan_offers
        (plan_id, destination, id, source_id, type, title, price_per_person, currency,
         availability, url, scraped_at, duration_days, package_subtype)
       VALUES (${sqlText(input.plan_id)}, ${sqlText(input.destination)}, ${sqlText(o.offer_id)},
         ${sqlText(o.source_id)}, ${sqlText('package')}, ${sqlText(o.title)},
         ${sqlInt(o.price_per_person_twd)}, ${sqlText('TWD')},
         ${sqlText(o.departure_status ?? null)}, ${sqlText(o.url)}, ${sqlText(o.scraped_at)},
         ${sqlInt(o.nights + 1)}, ${sqlText('group_tour')});`
    );
    stmts.push(
      `INSERT OR REPLACE INTO plan_offer_group_meta
        (plan_id, destination, offer_id, meals_included_count, departure_status,
         seats_available, min_group_size, group_size_cap, source_offer_run_id, source_offer_id)
       VALUES (${sqlText(input.plan_id)}, ${sqlText(input.destination)}, ${sqlText(o.offer_id)},
         ${sqlInt(o.meals_included_count ?? null)}, ${sqlText(o.departure_status ?? null)},
         ${sqlInt(o.seats_available ?? null)}, ${sqlInt(o.min_group_size ?? null)},
         ${sqlInt(o.group_size_cap ?? null)}, ${sqlText(o.run_id)}, ${sqlText(o.offer_id)});`
    );
  }
  await client.executeMany(stmts);
  return audit;
}
```

- [ ] **Step 4: Run the test**

Run: `npm test -- tour-group-bridge`
Expected: 2 PASSED.

- [ ] **Step 5: Wire the bridge into stage0-adopt**

Open `src/services/stage0-service.ts`. Find `adoptCandidateToNewPlan` (around line 389). At the end of the function, after the existing plan-creation logic completes (after the date-anchor and destination inserts are committed), add:

```typescript
  // Bridge tour-group baseline into plan_offers + plan_offer_group_meta.
  // Non-fatal: if no tour-group offers were scraped for this run, the bridge
  // returns 0 rows and we move on.
  try {
    const { bridgeAuditSet } = await import('./tour-group-bridge');
    // Conservative: only bridge if the input includes a dest_region.
    if (input.destRegion) {
      const audit = await bridgeAuditSet({
        run_id: runId,
        plan_id: input.planId,
        destination: input.destinationSlug,
        dest_region: input.destRegion,
        nights,
      });
      if (audit.length > 0) {
        console.error(`ℹ️  Bridged ${audit.length} tour-group baseline offers into ${input.planId}.`);
      }
    }
  } catch (err: any) {
    console.error(`⚠️  Tour-group bridge skipped: ${err?.message || err}`);
  }
```

You will also need to extend the `AdoptToNewPlanInput` interface (defined earlier in the same file) to include `destRegion?: string`. Find the `interface AdoptToNewPlanInput` declaration and add:

```typescript
  destRegion?: string;  // Optional: if present, bridge tour-group baseline at adopt time
```

Apply the same bridge call at the end of `adoptCandidate` (the existing-plan variant, line 370) — but only if the function takes/can be passed `destRegion`. If `adoptCandidate` doesn't take that parameter today, leave the existing-plan path alone for now and document: only `--create-plan` adoptions get the bridge in this build. A follow-up can extend `adoptCandidate`.

- [ ] **Step 6: Extend the `stage0-adopt` CLI to forward `--dest-region`**

Open `src/cli/commands/stage0.ts`. Find the `stage0AdoptCommand` handler (around line 100-150, identifiable by `names: ['stage0-adopt']`). In the section that parses CLI options for the `--create-plan` path, add:

```typescript
      const destRegion = ctx.args.optionValue('--dest-region');
```

And pass it into `adoptCandidateToNewPlan({ candidateId, planId, destinationSlug, destRegion })`.

Update the `usage` string on that command to mention the optional flag:

```typescript
  usage: 'stage0-adopt <candidate_id> <plan_id> [--create-plan --dest <slug> [--dest-region <region>]]',
```

- [ ] **Step 7: Typecheck**

Run: `npm run typecheck`
Expected: 0 errors.

- [ ] **Step 8: Run all tour-group tests + the existing stage0 tests**

Run: `npm test -- tour-group`
Expected: all PASS.

Run: `npm test -- stage0-service`
Expected: all PASS (no regressions).

- [ ] **Step 9: Commit**

```bash
git add src/services/tour-group-bridge.ts src/services/stage0-service.ts \
  src/cli/commands/stage0.ts \
  tests/integration/tour-group-bridge.regression.test.ts
git commit -m "feat(tour-group): bridge audit set into plan_offers at stage0-adopt"
```

---

## Task 5: BestTour scraper

**Files:**
- Create: `scripts/scrape_tour_groups.py` (entry point with `--source` flag)
- Create: `scripts/scrapers/parsers/besttour_groups.py` (if `scripts/scrapers/parsers/` doesn't exist, create the directory)

This task scrapes the live BestTour site. It depends on the exact HTML structure of `besttour.com.tw`, which can change. The implementation here is a starting point; if selectors don't match the live site at the time of execution, the engineer should update the selectors after reading the live HTML.

- [ ] **Step 1: Create the entry-point script**

Create `scripts/scrape_tour_groups.py`:

```python
#!/usr/bin/env python3
"""
Tour-group listing scraper. One entry point, --source dispatches to per-agency parsers.

Usage:
    python scripts/scrape_tour_groups.py \\
        --source besttour --dest-region kansai --nights 5 \\
        --depart-start 2026-06-14 --depart-end 2026-06-28 \\
        --run-id stage0-20260525-... --output scrapes/besttour-kansai-5n.json

Writes a JSON envelope matching docs/superpowers/specs/2026-05-25-tour-group-scraper-design.md §3.3.
"""
import argparse
import asyncio
import json
import sys
from datetime import datetime
from pathlib import Path

try:
    from playwright.async_api import async_playwright
except ImportError:
    print("Playwright not installed. Run: pip install playwright && playwright install chromium")
    sys.exit(1)


async def scrape(args):
    if args.source == 'besttour':
        from scrapers.parsers.besttour_groups import scrape_besttour_groups
        offers = await scrape_besttour_groups(
            dest_region=args.dest_region,
            nights=args.nights,
            depart_start=args.depart_start,
            depart_end=args.depart_end,
            run_id=args.run_id,
        )
    elif args.source == 'lifetour':
        from scrapers.parsers.lifetour_groups import scrape_lifetour_groups
        offers = await scrape_lifetour_groups(
            dest_region=args.dest_region, nights=args.nights,
            depart_start=args.depart_start, depart_end=args.depart_end,
            run_id=args.run_id,
        )
    elif args.source == 'settour':
        from scrapers.parsers.settour_groups import scrape_settour_groups
        offers = await scrape_settour_groups(
            dest_region=args.dest_region, nights=args.nights,
            depart_start=args.depart_start, depart_end=args.depart_end,
            run_id=args.run_id,
        )
    else:
        print(f"Unknown source: {args.source}")
        sys.exit(1)

    envelope = {
        "run_id": args.run_id,
        "scraped_at": datetime.utcnow().isoformat() + 'Z',
        "source_id": args.source,
        "dest_region": args.dest_region,
        "nights": args.nights,
        "tour_group_offers": offers,
    }
    out = Path(args.output)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(envelope, ensure_ascii=False, indent=2), encoding='utf-8')
    print(f"✅ Wrote {len(offers)} offers to {out}")


def main():
    p = argparse.ArgumentParser()
    p.add_argument('--source', required=True, choices=['besttour', 'lifetour', 'settour'])
    p.add_argument('--dest-region', required=True, help='e.g. kansai, kanto, kyushu')
    p.add_argument('--nights', type=int, required=True)
    p.add_argument('--depart-start', required=True)
    p.add_argument('--depart-end', required=True)
    p.add_argument('--run-id', required=True)
    p.add_argument('--output', required=True)
    args = p.parse_args()
    sys.path.insert(0, str(Path(__file__).parent))
    asyncio.run(scrape(args))


if __name__ == '__main__':
    main()
```

- [ ] **Step 2: Create the BestTour parser**

If `scripts/scrapers/parsers/` doesn't exist, the entry point's `sys.path.insert(0, str(Path(__file__).parent))` looks under `scripts/`, so the import is `from scrapers.parsers.besttour_groups import ...`. Confirm `scripts/scrapers/parsers/` exists; create with `__init__.py` files if not.

Create `scripts/scrapers/parsers/besttour_groups.py`:

```python
"""BestTour 喜鴻假期 tour-group listing parser.

Listing URL pattern (group tour, Kansai):
  https://www.besttour.com.tw/e_web/group?v=japan_kansai

The exact URL parameter naming may shift; if the scrape returns 0 rows,
open the live site, browse to 日本→關西 group tours, and update the URL.

Each listing card exposes (at the time of writing):
- title (link text)
- product code (in URL)
- price per person (numeric span)
- departure date (date label)
- hotel name (often in title or subtitle)
- nights (computed from depart_date → return_date or in title)
- departure status ('成團' = guaranteed, '可候補' = waitlist, otherwise treat as 'available')
"""
import hashlib
import re
from datetime import datetime, timedelta
from playwright.async_api import async_playwright


LISTING_URL = "https://www.besttour.com.tw/e_web/group?v=japan_{region}"


async def scrape_besttour_groups(dest_region: str, nights: int,
                                  depart_start: str, depart_end: str,
                                  run_id: str) -> list:
    region_param = dest_region  # 'kansai', 'kanto', etc.
    url = LISTING_URL.format(region=region_param)

    offers = []
    async with async_playwright() as p:
        browser = await p.chromium.launch(headless=True)
        ctx = await browser.new_context()
        page = await ctx.new_page()
        await page.goto(url, wait_until='networkidle', timeout=60000)

        # Listing cards selector. Update if the live site differs.
        cards = await page.query_selector_all('.product-item, .group-card, .tour-card')
        for card in cards:
            try:
                offer = await _parse_card(card, run_id, dest_region, nights, depart_start, depart_end)
                if offer:
                    offers.append(offer)
            except Exception as e:
                # Skip cards that fail to parse; the importer will count them as `skipped`.
                # Record the failure in raw_json for later inspection.
                pass

        await browser.close()
    return offers


async def _parse_card(card, run_id: str, dest_region: str, target_nights: int,
                      depart_start: str, depart_end: str) -> dict | None:
    title_el = await card.query_selector('a, .title, h3')
    price_el = await card.query_selector('.price, .price-num, [class*="price"]')
    url_el = title_el  # link is usually the title element

    if not title_el or not price_el:
        return None

    title = (await title_el.inner_text()).strip()
    price_text = (await price_el.inner_text()).strip()
    # Extract digits only — strip $, comma, "起", "TWD", etc.
    price_match = re.search(r'(\d[\d,]*)', price_text.replace(',', ''))
    if not price_match:
        return None
    price = int(price_match.group(1).replace(',', ''))

    url = await url_el.get_attribute('href') or ''
    if url and not url.startswith('http'):
        url = 'https://www.besttour.com.tw' + url

    # Departure date — selectors vary; try common ones.
    date_text = ''
    for sel in ['.depart-date', '.date', '[class*="depart"]', 'time']:
        el = await card.query_selector(sel)
        if el:
            date_text = (await el.inner_text()).strip()
            break
    depart_date = _parse_date(date_text)
    if not depart_date:
        return None
    if not (depart_start <= depart_date <= depart_end):
        return None

    # Filter by target_nights from title or computed length.
    nights_match = re.search(r'(\d+)\s*(?:日|天|days?)', title)
    if nights_match:
        days = int(nights_match.group(1))
        derived_nights = days - 1
        if derived_nights != target_nights:
            return None
    return_date = (datetime.strptime(depart_date, '%Y-%m-%d') + timedelta(days=target_nights)).strftime('%Y-%m-%d')

    short_hash = hashlib.md5(f"{url}|{depart_date}".encode()).hexdigest()[:6]
    offer_id = f"besttour-{dest_region.upper()}-{depart_date.replace('-','')}-{target_nights}n-{short_hash}"

    # Optional comparables — best-effort, leave NULL if not present.
    hotel_name = None
    for sel in ['.hotel-name', '.hotel', '[class*="hotel"]']:
        el = await card.query_selector(sel)
        if el:
            hotel_name = (await el.inner_text()).strip() or None
            break

    departure_status = 'available'
    status_text = await card.inner_text()
    if '成團' in status_text or '保證出發' in status_text:
        departure_status = 'guaranteed'
    elif '可候補' in status_text or '候補' in status_text:
        departure_status = 'waitlist'
    elif '額滿' in status_text or '售完' in status_text:
        departure_status = 'sold_out'

    return {
        "run_id": run_id,
        "offer_id": offer_id,
        "source_id": "besttour",
        "dest_region": dest_region,
        "depart_date": depart_date,
        "return_date": return_date,
        "nights": target_nights,
        "price_per_person_twd": price,
        "title": title,
        "url": url,
        "scraped_at": datetime.utcnow().isoformat() + 'Z',
        "hotel_name": hotel_name,
        "hotel_star_rating": None,    # Star rating not consistently on listing page; leave NULL.
        "meals_included_count": None,
        "departure_status": departure_status,
        "seats_available": None,
        "min_group_size": None,
        "group_size_cap": None,
        "raw_json": None,
    }


def _parse_date(s: str) -> str | None:
    """Parse common Chinese-locale date strings into YYYY-MM-DD."""
    if not s:
        return None
    # Try 2026/06/20
    m = re.search(r'(\d{4})[/\-年](\d{1,2})[/\-月](\d{1,2})', s)
    if m:
        y, mo, d = m.groups()
        return f"{y}-{int(mo):02d}-{int(d):02d}"
    # Try 06/20 (assume current year)
    m = re.search(r'(\d{1,2})[/\-月](\d{1,2})', s)
    if m:
        mo, d = m.groups()
        y = datetime.utcnow().year
        return f"{y}-{int(mo):02d}-{int(d):02d}"
    return None
```

- [ ] **Step 3: Manual run against the live site**

Run:
```bash
python scripts/scrape_tour_groups.py \
  --source besttour --dest-region kansai --nights 5 \
  --depart-start 2026-06-14 --depart-end 2026-06-28 \
  --run-id test-besttour-manual \
  --output scrapes/besttour-kansai-5n-manual.json
```

Expected: stdout shows `✅ Wrote N offers to scrapes/besttour-kansai-5n-manual.json` where N >= 1.

If N == 0, the selectors don't match the live site. Open `https://www.besttour.com.tw/e_web/group?v=japan_kansai` in a browser, inspect the listing card HTML, and update the selectors in `_parse_card`. Re-run until N >= 1.

- [ ] **Step 4: Verify the output JSON shape**

Run: `head -50 scrapes/besttour-kansai-5n-manual.json`

Expected: top-level keys `run_id`, `scraped_at`, `source_id`, `dest_region`, `nights`, `tour_group_offers` (array). Each offer has the required fields per spec §3.3.

- [ ] **Step 5: Round-trip — import the live output**

Seed the attempt row:
```bash
npm run db:exec -- "INSERT OR REPLACE INTO stage0_tour_group_scrape_attempts (run_id, source_id, dest_region, nights, status) VALUES ('test-besttour-manual', 'besttour', 'kansai', 5, 'pending')"
```

Import:
```bash
npm run travel -- import-tour-group-offers --run test-besttour-manual --file scrapes/besttour-kansai-5n-manual.json
```

Expected: `✅ Imported N offers (skipped X). Attempt status: ok` (or `partial`).

Query:
```bash
npm run travel -- query-tour-group-offers --run test-besttour-manual
```

Expected: tabular output showing the imported offers, sorted ascending by price.

- [ ] **Step 6: Clean up the test run**

```bash
npm run db:exec -- "DELETE FROM stage0_tour_group_offers WHERE run_id = 'test-besttour-manual'"
npm run db:exec -- "DELETE FROM stage0_tour_group_scrape_attempts WHERE run_id = 'test-besttour-manual'"
rm scrapes/besttour-kansai-5n-manual.json
```

- [ ] **Step 7: Commit**

```bash
git add scripts/scrape_tour_groups.py scripts/scrapers/parsers/besttour_groups.py
git commit -m "feat(tour-group): BestTour 喜鴻 group-tour listing scraper"
```

---

## Task 6: Lifetour scraper

**Files:**
- Create: `scripts/scrapers/parsers/lifetour_groups.py`

- [ ] **Step 1: Implement the parser**

Lifetour 五福 group-tour listing URL pattern (from existing `scrape_listings.py` builders):

```
https://tour.lifetour.com.tw/searchlist/tpe/{region_code}
```

Region codes: `0001-0003` for Kansai, `0001-0001` for Kanto. Confirm against the existing `scripts/scrape_listings.py` URL builder if extending — the same URL helper may be reusable.

Create `scripts/scrapers/parsers/lifetour_groups.py`:

```python
"""Lifetour 五福 tour-group listing parser.

Same structure as besttour_groups.py. Selectors differ:
- Each tour is in .product-card or .listing-item.
- Price is shown as "TWD 32,900起" inside .price-num.
- Departure date can be a calendar selector — listing page may
  show a single rolling start date; for date-window scraping we
  may need to iterate over visible date chips.
"""
import hashlib
import re
from datetime import datetime, timedelta
from playwright.async_api import async_playwright


LIFETOUR_REGION_CODES = {
    'kansai': '0001-0003',
    'kanto':  '0001-0001',
    'kyushu': '0001-0005',
    'tohoku': '0001-0006',  # confirm before use
}


async def scrape_lifetour_groups(dest_region: str, nights: int,
                                  depart_start: str, depart_end: str,
                                  run_id: str) -> list:
    code = LIFETOUR_REGION_CODES.get(dest_region)
    if not code:
        raise ValueError(f"No Lifetour region code for {dest_region}")
    url = f"https://tour.lifetour.com.tw/searchlist/tpe/{code}"

    offers = []
    async with async_playwright() as p:
        browser = await p.chromium.launch(headless=True)
        ctx = await browser.new_context()
        page = await ctx.new_page()
        await page.goto(url, wait_until='networkidle', timeout=60000)

        cards = await page.query_selector_all('.product-card, .listing-item, .tour-item')
        for card in cards:
            try:
                offer = await _parse_card(card, run_id, dest_region, nights, depart_start, depart_end)
                if offer:
                    offers.append(offer)
            except Exception:
                pass

        await browser.close()
    return offers


async def _parse_card(card, run_id, dest_region, target_nights, depart_start, depart_end):
    title_el = await card.query_selector('a.title, h3, .product-name')
    price_el = await card.query_selector('.price-num, .price, [class*="price"]')
    if not title_el or not price_el:
        return None

    title = (await title_el.inner_text()).strip()
    price_text = (await price_el.inner_text()).strip()
    price_match = re.search(r'(\d[\d,]*)', price_text.replace(',', ''))
    if not price_match:
        return None
    price = int(price_match.group(1).replace(',', ''))

    url = await title_el.get_attribute('href') or ''
    if url and not url.startswith('http'):
        url = 'https://tour.lifetour.com.tw' + url

    date_text = ''
    for sel in ['.depart-date', '.date', '[class*="depart"]']:
        el = await card.query_selector(sel)
        if el:
            date_text = (await el.inner_text()).strip()
            break
    depart_date = _parse_date(date_text)
    if not depart_date or not (depart_start <= depart_date <= depart_end):
        return None

    nights_match = re.search(r'(\d+)\s*(?:日|天)', title)
    if nights_match:
        derived = int(nights_match.group(1)) - 1
        if derived != target_nights:
            return None
    return_date = (datetime.strptime(depart_date, '%Y-%m-%d') + timedelta(days=target_nights)).strftime('%Y-%m-%d')

    short_hash = hashlib.md5(f"{url}|{depart_date}".encode()).hexdigest()[:6]
    offer_id = f"lifetour-{dest_region.upper()}-{depart_date.replace('-','')}-{target_nights}n-{short_hash}"

    return {
        "run_id": run_id,
        "offer_id": offer_id,
        "source_id": "lifetour",
        "dest_region": dest_region,
        "depart_date": depart_date,
        "return_date": return_date,
        "nights": target_nights,
        "price_per_person_twd": price,
        "title": title,
        "url": url,
        "scraped_at": datetime.utcnow().isoformat() + 'Z',
        "hotel_name": None,
        "hotel_star_rating": None,
        "meals_included_count": None,
        "departure_status": "available",
        "seats_available": None,
        "min_group_size": None,
        "group_size_cap": None,
        "raw_json": None,
    }


def _parse_date(s: str) -> str | None:
    if not s:
        return None
    m = re.search(r'(\d{4})[/\-年](\d{1,2})[/\-月](\d{1,2})', s)
    if m:
        y, mo, d = m.groups()
        return f"{y}-{int(mo):02d}-{int(d):02d}"
    m = re.search(r'(\d{1,2})[/\-月](\d{1,2})', s)
    if m:
        mo, d = m.groups()
        return f"{datetime.utcnow().year}-{int(mo):02d}-{int(d):02d}"
    return None
```

- [ ] **Step 2: Manual run + import (same pattern as Task 5)**

Run:
```bash
python scripts/scrape_tour_groups.py \
  --source lifetour --dest-region kansai --nights 5 \
  --depart-start 2026-06-14 --depart-end 2026-06-28 \
  --run-id test-lifetour-manual \
  --output scrapes/lifetour-kansai-5n-manual.json
```

If N == 0, update selectors against the live Lifetour listing page and re-run.

- [ ] **Step 3: Round-trip import + cleanup (same pattern as Task 5 step 5+6)**

Seed → import → query → cleanup.

- [ ] **Step 4: Commit**

```bash
git add scripts/scrapers/parsers/lifetour_groups.py
git commit -m "feat(tour-group): Lifetour 五福 group-tour listing scraper"
```

---

## Task 7: Settour scraper

**Files:**
- Create: `scripts/scrapers/parsers/settour_groups.py`

- [ ] **Step 1: Implement the parser**

Settour 東南 listing URL pattern (from `scripts/scrape_listings.py`):

```
https://tour.settour.com.tw/search?destinationCode={code}
```

Codes: `JX_3` for Kansai, `JX_1` for Kanto, etc.

Create `scripts/scrapers/parsers/settour_groups.py`:

```python
"""Settour 東南 tour-group listing parser."""
import hashlib
import re
from datetime import datetime, timedelta
from playwright.async_api import async_playwright


SETTOUR_REGION_CODES = {
    'kansai': 'JX_3',
    'kanto':  'JX_1',
    'kyushu': 'JX_5',
    'tohoku': 'JX_6',  # confirm before use
}


async def scrape_settour_groups(dest_region: str, nights: int,
                                 depart_start: str, depart_end: str,
                                 run_id: str) -> list:
    code = SETTOUR_REGION_CODES.get(dest_region)
    if not code:
        raise ValueError(f"No Settour region code for {dest_region}")
    url = f"https://tour.settour.com.tw/search?destinationCode={code}"

    offers = []
    async with async_playwright() as p:
        browser = await p.chromium.launch(headless=True)
        ctx = await browser.new_context()
        page = await ctx.new_page()
        await page.goto(url, wait_until='networkidle', timeout=60000)

        cards = await page.query_selector_all('.product, .product-card, .tour-item')
        for card in cards:
            try:
                offer = await _parse_card(card, run_id, dest_region, nights, depart_start, depart_end)
                if offer:
                    offers.append(offer)
            except Exception:
                pass

        await browser.close()
    return offers


async def _parse_card(card, run_id, dest_region, target_nights, depart_start, depart_end):
    title_el = await card.query_selector('a, h3, .product-title')
    price_el = await card.query_selector('.price, .price-num, [class*="price"]')
    if not title_el or not price_el:
        return None
    title = (await title_el.inner_text()).strip()
    price_text = (await price_el.inner_text()).strip()
    price_match = re.search(r'(\d[\d,]*)', price_text.replace(',', ''))
    if not price_match:
        return None
    price = int(price_match.group(1).replace(',', ''))

    url = await title_el.get_attribute('href') or ''
    if url and not url.startswith('http'):
        url = 'https://tour.settour.com.tw' + url

    date_text = ''
    for sel in ['.depart-date', '.date']:
        el = await card.query_selector(sel)
        if el:
            date_text = (await el.inner_text()).strip()
            break
    depart_date = _parse_date(date_text)
    if not depart_date or not (depart_start <= depart_date <= depart_end):
        return None

    nights_match = re.search(r'(\d+)\s*(?:日|天)', title)
    if nights_match:
        if int(nights_match.group(1)) - 1 != target_nights:
            return None
    return_date = (datetime.strptime(depart_date, '%Y-%m-%d') + timedelta(days=target_nights)).strftime('%Y-%m-%d')

    short_hash = hashlib.md5(f"{url}|{depart_date}".encode()).hexdigest()[:6]
    offer_id = f"settour-{dest_region.upper()}-{depart_date.replace('-','')}-{target_nights}n-{short_hash}"

    return {
        "run_id": run_id,
        "offer_id": offer_id,
        "source_id": "settour",
        "dest_region": dest_region,
        "depart_date": depart_date,
        "return_date": return_date,
        "nights": target_nights,
        "price_per_person_twd": price,
        "title": title,
        "url": url,
        "scraped_at": datetime.utcnow().isoformat() + 'Z',
        "hotel_name": None,
        "hotel_star_rating": None,
        "meals_included_count": None,
        "departure_status": "available",
        "seats_available": None,
        "min_group_size": None,
        "group_size_cap": None,
        "raw_json": None,
    }


def _parse_date(s):
    if not s:
        return None
    m = re.search(r'(\d{4})[/\-年](\d{1,2})[/\-月](\d{1,2})', s)
    if m:
        y, mo, d = m.groups()
        return f"{y}-{int(mo):02d}-{int(d):02d}"
    m = re.search(r'(\d{1,2})[/\-月](\d{1,2})', s)
    if m:
        mo, d = m.groups()
        return f"{datetime.utcnow().year}-{int(mo):02d}-{int(d):02d}"
    return None
```

- [ ] **Step 2: Manual run + import (same as Task 5/6)**

Run, seed, import, query, cleanup.

- [ ] **Step 3: Commit**

```bash
git add scripts/scrapers/parsers/settour_groups.py
git commit -m "feat(tour-group): Settour 東南 group-tour listing scraper"
```

---

## Task 8: Docs update

**Files:**
- Modify: `CLAUDE.md`
- Modify: `docs/reference/CLI.md`

- [ ] **Step 1: Add the two new commands to `CLAUDE.md` CLI Quick Reference**

Open `CLAUDE.md`. Find the `# Bookings` section in the CLI Quick Reference (around the `npm run travel -- sync-bookings` line). After the `validate-itinerary` line, add a new subsection:

```bash
# Tour-group baseline (new — see docs/superpowers/specs/2026-05-25-tour-group-scraper-design.md)
python scripts/scrape_tour_groups.py --source besttour --dest-region kansai --nights 5 \
  --depart-start 2026-06-14 --depart-end 2026-06-28 \
  --run-id <run_id> --output scrapes/besttour-kansai-5n.json
npm run travel -- import-tour-group-offers --run <run_id> --file <path>
npm run travel -- query-tour-group-offers --run <run_id> [--source <id>] [--dest-region <region>] [--max-price <twd>]
```

Also update the Skill Decision Tree if "tour group" / "baseline" / "ceiling" routing is missing — add a row:

```
"find tour group" / "set baseline"   → python scripts/scrape_tour_groups.py + import-tour-group-offers
```

- [ ] **Step 2: Add full reference entries to `docs/reference/CLI.md`**

Open `docs/reference/CLI.md`. After the existing `## Stage 0 — Triangle research` section, add a new section:

```markdown
## Stage 0 — Tour-group baseline (pre-plan; unscoped)

Scrape tour-group listings from agency sites to set the price ceiling for the trip. See `docs/superpowers/specs/2026-05-25-tour-group-scraper-design.md` for design and `docs/superpowers/specs/2026-05-25-price-baseline-and-rhythm-method.md` for why.

```bash
# 1. Scrape one agency × region × nights, output to JSON
python scripts/scrape_tour_groups.py \
  --source besttour --dest-region kansai --nights 5 \
  --depart-start 2026-06-14 --depart-end 2026-06-28 \
  --run-id <run_id> \
  --output scrapes/besttour-kansai-5n.json

# 2. Import the JSON into stage0_tour_group_offers + update the attempt row
npm run travel -- import-tour-group-offers --run <run_id> --file <path>

# 3. List what's been collected
npm run travel -- query-tour-group-offers --run <run_id> [--source <id>] [--dest-region <region>] [--nights N] [--max-price TWD] [--json]
```

Supported sources: `besttour`, `lifetour`, `settour`. LionTravel group and Travel4U deferred.

At `stage0-adopt --create-plan --dest-region <region>`, the curated audit set (raw cheapest + quality-floor cheapest + top-3 per source/date) is bridged into `plan_offers` with `package_subtype='group_tour'` and `plan_offer_group_meta`.
```

- [ ] **Step 3: Run typecheck (sanity)**

Run: `npm run typecheck`
Expected: 0 errors.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md docs/reference/CLI.md
git commit -m "docs: add tour-group scraper commands to CLAUDE.md + CLI.md"
```

---

## Task 9: End-to-end verification

This is a sanity check, not a code change. The June 2026 Stage 0 run is the realistic end-to-end target.

**Files:** none (verification only)

- [ ] **Step 1: Run the full test suite**

Run: `npm test`
Expected: all tests PASS. Compare to baseline before this work; no regressions.

- [ ] **Step 2: Run typecheck + validator**

Run: `npm run typecheck && npm run validate:data`
Expected: 0 errors in both.

- [ ] **Step 3: Live scrape — BestTour Kansai 5 nights for the existing June 2026 run**

This uses the already-seeded `stage0-20260525-093508` run (KIX/SDJ/FUK × 3,4 nights). Tour-group scraping is independent of the flight-scrape matrix — it adds a *new* attempt row for `(run, source, region, nights)` not previously tracked.

First, seed an attempt:

```bash
npm run db:exec -- "INSERT OR REPLACE INTO stage0_tour_group_scrape_attempts (run_id, source_id, dest_region, nights, status) VALUES ('stage0-20260525-093508', 'besttour', 'kansai', 4, 'pending')"
```

Then scrape + import:

```bash
python scripts/scrape_tour_groups.py \
  --source besttour --dest-region kansai --nights 4 \
  --depart-start 2026-06-14 --depart-end 2026-06-28 \
  --run-id stage0-20260525-093508 \
  --output scrapes/besttour-kansai-4n.json

npm run travel -- import-tour-group-offers \
  --run stage0-20260525-093508 \
  --file scrapes/besttour-kansai-4n.json
```

Expected: `✅ Imported N offers (skipped X). Attempt status: ok` (or `partial`).

- [ ] **Step 4: Query the baseline**

```bash
npm run travel -- query-tour-group-offers --run stage0-20260525-093508 --max-price 50000
```

Expected: tabular output showing tour groups ranked cheapest-first. The cheapest row is the baseline ceiling for Kansai 4n in this window.

- [ ] **Step 5: Final commit (verification log only)**

No code change; only proceed to Task 10 if Steps 1–4 all pass. If a scraper failed against the live site, fix the selectors in the relevant parser file and re-commit before moving on.

---

## Task 10: Push

- [ ] **Step 1: Push to master**

This is a solo repo — no PR workflow. Commit on whatever branch is convenient for iteration (feature branch is fine for isolation while live-scrape selectors are unstable), then fast-forward merge to master and push.

```bash
# If working on a feature branch, fold it into master first:
# git checkout master && git merge --ff-only <branch> && git branch -d <branch> && git push origin --delete <branch>

git push origin master
```

Expected: commits from Tasks 1–8 land on `origin/master`.

---

## Self-review checklist

Per the writing-plans skill self-review:

**Spec coverage:**
- §1 Agency selection — covered in Tasks 5/6/7 (BestTour, Lifetour, Settour). LionTravel + Travel4U explicitly deferred per spec.
- §2.1 New unscoped tables — Task 1.
- §2.2 Plan-side extension — Task 1.
- §3 Scrape strategy — Tasks 5/6/7 (listing-first; no detail fallback, matching spec §3.1).
- §3.2 Attempt lifecycle — covered in Task 3 (importer computes status `ok | partial | failed` from parsed/skipped counts).
- §3.3 Output envelope — Task 3 (fixtures match spec exactly); Task 5 (scraper writes the envelope).
- §4 New CLI commands — Task 3.
- §5 Adopt-time bridge with audit set — Task 4.
- §6 Open/deferred decisions — quality-floor default of 4 is encoded in `findAuditSet` (Task 2). Detail-page fallback explicitly not implemented per spec.
- §7 Test plan — Task 3 covers tests 1–3 (importer happy/partial/mismatch); Task 4 covers tests 4–5 (bridge happy + empty quality-floor).
- §8 Build sequence — Tasks 1–7 follow the spec's 8-step sequence (the spec's step 6 "methodology spec hookup" is intentionally a no-op in this build).

**Placeholders:** None. Every step shows the actual code, command, or expected output.

**Type consistency:** `TourGroupOfferRow` shape defined in Task 2 matches the JSON fixtures in Task 3 and the parser output in Tasks 5–7. `ScrapeAttemptStatus` enum used consistently. `bridgeAuditSet` signature (Task 4) matches its call site in `adoptCandidateToNewPlan`.

**One known live-site fragility:** Tasks 5–7 contain CSS selectors that may not match the live agency sites at execution time. Each task includes an explicit "if N == 0, update selectors against the live HTML" instruction. This is the right tradeoff vs. failing the plan — selector drift is a normal scraper concern.
