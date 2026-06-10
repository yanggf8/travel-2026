/**
 * StateManagerV2 — fine-grained DB ops (ADR-001) integration test.
 *
 * Proves the target pattern against a REAL Turso DB (no mocks, per ADR-001
 * "Testing"): seed a minimal `days` row → call the V2 method → SELECT the row
 * → assert → tear down. If Turso creds are absent, the suite skips rather than
 * failing (keeps credless runs green).
 *
 * Pattern reference: src/skills/travel-shared/references/architecture-decisions.md
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import { StateManager } from '../../src/state/state-manager';
import { TursoDbClient } from '../../src/state/turso-db-client';
import { arg } from '../../src/state/db-client';

const TEST_PLAN_ID = 'test-smv2-daytheme';
const TEST_DEST = 'tokyo_2026';

// Creds come from .env. Load it into process.env up front (same as the pipeline
// client does), then decide whether to run. Skip the suite if absent.
function loadEnvAndCheckCreds(): boolean {
  try {
    const fs = require('node:fs');
    const path = require('node:path');
    const envPath = path.join(process.cwd(), '.env');
    if (fs.existsSync(envPath)) {
      const raw: string = fs.readFileSync(envPath, 'utf-8');
      for (const line of raw.split('\n')) {
        const t = line.trim();
        if (!t || t.startsWith('#')) continue;
        const eq = t.indexOf('=');
        if (eq === -1) continue;
        const key = t.slice(0, eq).trim();
        let val = t.slice(eq + 1).trim();
        if ((val.startsWith('"') && val.endsWith('"')) || (val.startsWith("'") && val.endsWith("'"))) {
          val = val.slice(1, -1);
        }
        if (!process.env[key] && val) process.env[key] = val;
      }
    }
  } catch {
    /* ignore */
  }
  return Boolean(process.env.TURSO_TOKEN && (process.env.TURSO_URL || process.env.TURSO_HTTP_ENDPOINT));
}

const hasCreds = loadEnvAndCheckCreds();

describe.runIf(hasCreds)('StateManagerV2: setDayThemeV2 (real Turso, ADR-001)', () => {
  let db: TursoDbClient;
  let sm: StateManager;

  async function cleanup(): Promise<void> {
    await db.execute('DELETE FROM days WHERE plan_id = ?', [arg.text(TEST_PLAN_ID)]);
  }

  beforeAll(async () => {
    db = new TursoDbClient();
    await cleanup();
    // Seed one minimal day row (day_type/date/status are NOT NULL per schema).
    await db.execute(
      `INSERT INTO days (plan_id, destination, day_number, date, day_type, status)
       VALUES (?, ?, ?, ?, 'arrival', 'draft')`,
      [arg.text(TEST_PLAN_ID), arg.text(TEST_DEST), arg.int(1), arg.text('2026-02-13')]
    );

    // V2 path: inject the same client; force the plan id to the seeded key.
    sm = await StateManager.create({ plan: minimalPlan(), skipSave: true, dbClient: db });
    (sm as unknown as { planId: string }).planId = TEST_PLAN_ID;
  });

  afterAll(async () => {
    if (db) await cleanup();
  });

  it('updates theme + theme_zh via targeted UPDATE and reports 1 affected row', async () => {
    const affected = await sm.setDayThemeV2(TEST_DEST, 1, 'Arrival Day', '抵達日');
    expect(affected).toBe(1);

    const row = await db.queryOne<{ theme: string; theme_zh: string }>(
      'SELECT theme, theme_zh FROM days WHERE plan_id = ? AND destination = ? AND day_number = ?',
      [arg.text(TEST_PLAN_ID), arg.text(TEST_DEST), arg.int(1)]
    );
    expect(row?.theme).toBe('Arrival Day');
    expect(row?.theme_zh).toBe('抵達日');
  });

  it('leaves theme_zh untouched when omitted (partial update)', async () => {
    await sm.setDayThemeV2(TEST_DEST, 1, 'Full Day'); // themeZh undefined
    const row = await db.queryOne<{ theme: string; theme_zh: string }>(
      'SELECT theme, theme_zh FROM days WHERE plan_id = ? AND destination = ? AND day_number = ?',
      [arg.text(TEST_PLAN_ID), arg.text(TEST_DEST), arg.int(1)]
    );
    expect(row?.theme).toBe('Full Day');
    expect(row?.theme_zh).toBe('抵達日'); // unchanged from previous test
  });

  it('throws when the day does not exist (SELECT-validate guard)', async () => {
    await expect(sm.setDayThemeV2(TEST_DEST, 99, 'Nope')).rejects.toThrow(/Day D99 not found/);
  });
});

/** Minimal in-memory plan to satisfy the StateManager constructor (skipSave mode). */
function minimalPlan() {
  return {
    schema_version: '4.2.0',
    active_destination: TEST_DEST,
    process_1_date_anchor: { status: 'confirmed' as const, start_date: '2026-02-13', end_date: '2026-02-17', num_days: 5, pax: 2 },
    destinations: {
      [TEST_DEST]: {
        slug: TEST_DEST,
        process_2_destination: { status: 'confirmed' as const },
      },
    },
  } as unknown as import('../../src/state/types').TravelPlanMinimal;
}
