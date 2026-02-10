# Travel Plan to Turso DB - Import Summary

## ✅ Completed Tasks

### 1. Created Export Script
**File**: `scripts/export-travel-plan-to-scrape.ts`

Converts travel-plan.json packages to scrape format compatible with Turso import:
- Extracts offers from `process_3_4_packages.results.offers`
- Adds `best_value` field for date extraction
- Outputs to `data/trips/{destination}/{destination}-packages-scrape.json`

### 2. Exported Osaka-Kyoto Packages
**Output**: `data/trips/osaka-kyoto-2026/osaka_kyoto_2026-packages-scrape.json`

Exported 2 packages:
- LionTravel APA Kyoto Ekimae (NT$21,796/person)
- Lifetour Hotel Tavinos Kyoto (NT$25,990/person)

### 3. Imported to Turso Database
**Method**: `npm run db:import:turso`

Initial import created records but with NULL departure_date due to schema constraint (PRIMARY KEY on `id` prevents multiple versions).

### 4. Updated Records with Dates
**Method**: Direct SQL UPDATE

Updated both offers with:
- `departure_date`: 2026-02-24
- `return_date`: 2026-02-28
- `nights`: 4
- `airline`: Thai Lion Air / Peach

## 📊 Verification

Query results:
```bash
npm run db:query:turso -- --destination osaka_kyoto_2026 --start 2026-02-24 --end 2026-02-28
```

Output:
```
Found 2 offer(s):

SOURCE      TYPE     PRICE     DATE                   NAME
--------------------------------------------------------------------------------
liontravel  package  TWD 21796 2026-02-24→2026-02-28  APA Hotel Kyoto Ekimae (APA京都站前)
lifetour    package  TWD 25990 2026-02-24→2026-02-28  HOTEL TAVINOS KYOTO (京都塔威諾斯飯店)
```

## 🔧 Usage

### Export from travel-plan.json
```bash
npx ts-node scripts/export-travel-plan-to-scrape.ts
```

### Import to Turso
```bash
npm run db:import:turso -- --files data/trips/osaka-kyoto-2026/osaka_kyoto_2026-packages-scrape.json --destination osaka_kyoto_2026 --region kansai
```

### Query offers
```bash
# By destination and date range
npm run db:query:turso -- --destination osaka_kyoto_2026 --start 2026-02-24 --end 2026-02-28

# Direct SQL
./scripts/turso-query.sh "SELECT * FROM offers WHERE destination = 'osaka_kyoto_2026'"
```

## ⚠️ Known Issues

### Schema Constraint
The `offers` table has `id TEXT PRIMARY KEY`, which prevents storing multiple versions of the same offer with different `scraped_at` timestamps. The `ON CONFLICT(id, scraped_at) DO NOTHING` clause in the import script doesn't work as intended because there's no unique constraint on `(id, scraped_at)`.

**Workaround**: Use UPDATE instead of INSERT for existing offers, or change the schema to allow versioning.

### Recommended Schema Change
```sql
-- Remove PRIMARY KEY from id
-- Add composite unique constraint
ALTER TABLE offers DROP CONSTRAINT offers_id_pkey;
CREATE UNIQUE INDEX offers_id_scraped_at_idx ON offers(id, scraped_at);
```

## 📝 Files Created/Modified

- ✅ `scripts/export-travel-plan-to-scrape.ts` - Export script
- ✅ `scripts/debug-import.ts` - Debug helper
- ✅ `data/trips/osaka-kyoto-2026/osaka_kyoto_2026-packages-scrape.json` - Exported data
- ✅ Turso DB `offers` table - 2 records updated

## 🎯 Next Steps

1. **Schema Migration** (optional): Update offers table to support versioning
2. **Automated Sync**: Add npm script to sync travel-plan → Turso on updates
3. **Event Logging**: Use `--events` flag to track imports in events table
4. **Booking Sync**: Sync selected package to bookings table

## 📚 Related Commands

```bash
# Database status
npm run db:status:turso

# Migrate schema
npm run db:migrate:turso

# Sync destinations
npm run db:sync:destinations

# Sync events
npm run db:sync:events
```
