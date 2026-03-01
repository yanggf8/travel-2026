#!/usr/bin/env tsx
// Rename itinerary_days → days, itinerary_sessions → timesofday
// Run AFTER all code references have been updated (C2 committed).
import * as fs from 'fs';
const envFile = '/home/yanggf/b/travel-2026/.env';
for (const line of fs.readFileSync(envFile, 'utf8').split('\n')) {
  const m = line.match(/^([^#=]+)=(.*)$/);
  if (m) process.env[m[1].trim()] = m[2].trim();
}
const TURSO_URL = process.env.TURSO_URL!.replace('libsql://', 'https://');
const TURSO_TOKEN = process.env.TURSO_TOKEN!;

const requests = [
  { type: 'execute', stmt: { sql: 'ALTER TABLE itinerary_days RENAME TO days' } },
  { type: 'execute', stmt: { sql: 'ALTER TABLE itinerary_sessions RENAME TO timesofday' } },
  { type: 'close' },
];

async function run() {
  const res = await fetch(`${TURSO_URL}/v2/pipeline`, {
    method: 'POST',
    headers: { Authorization: `Bearer ${TURSO_TOKEN}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ requests }),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}: ${await res.text()}`);
  const json = await res.json() as any;
  const labels = ['days rename', 'timesofday rename'];
  json.results?.slice(0, -1).forEach((r: any, i: number) => {
    const error = r?.response?.error;
    if (error) console.error(`❌ ${labels[i]}: ${JSON.stringify(error)}`);
    else console.log(`✅ ${labels[i]}`);
  });
}
run().catch(e => { console.error(e); process.exit(1); });
