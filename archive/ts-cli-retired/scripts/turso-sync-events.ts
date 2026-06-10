import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { TursoPipelineClient } from './turso-pipeline';

function sqlEscapeText(value: string): string {
  return value.replace(/'/g, "''");
}

function sqlText(value: string | null): string {
  if (value === null) return 'NULL';
  return `'${sqlEscapeText(value)}'`;
}

/**
 * Generate a unique ID for an event to avoid duplicates in Turso.
 * Since the events in state.json don't have IDs, we hash their content.
 */
function eventId(event: any): string {
  const payload = JSON.stringify({
    at: event.at,
    event: event.event,
    destination: event.destination,
    process: event.process,
    data: event.data
  });
  return crypto.createHash('sha1').update(payload).digest('hex');
}

async function main(): Promise<void> {
  const statePath = path.join(process.cwd(), 'data/state.json');
  if (!fs.existsSync(statePath)) {
    console.error(`State file not found: ${statePath}`);
    process.exit(1);
  }

  const state = JSON.parse(fs.readFileSync(statePath, 'utf-8'));
  const eventLog = state.event_log || [];

  if (eventLog.length === 0) {
    console.log('No events in event log.');
    return;
  }

  const client = new TursoPipelineClient();
  const sqlStatements: string[] = [];

  // Idempotency: Use external_id (hash of event content) to prevent duplicates.
  console.log(`Preparing to sync ${eventLog.length} events...`);

  for (const event of eventLog) {
    const eid = eventId(event);
    const cols = ['external_id', 'event_type', 'destination', 'process', 'data', 'created_at'];
    const values = [
      sqlText(eid),
      sqlText(event.event),
      sqlText(event.destination || null),
      sqlText(event.process || null),
      sqlText(event.data ? JSON.stringify(event.data) : null),
      sqlText(event.at),
    ];

    sqlStatements.push(
      `INSERT INTO events (${cols.join(',')}) VALUES (${values.join(',')}) ON CONFLICT(external_id) DO NOTHING;`
    );
  }

  // Idempotent sync: we can just send all statements and let Turso handle conflicts,
  // or we can still do a quick timestamp filter to reduce payload size.
  let syncStatements = sqlStatements;
  try {
    const statusRes = await client.execute('SELECT MAX(created_at) as v FROM events WHERE process != "turso_import"');
    const lastSyncedAt = (statusRes.results?.[0]?.response?.result?.rows?.[0] as any)?.[0];
    
    if (lastSyncedAt) {
      const lastAt = typeof lastSyncedAt === 'object' ? lastSyncedAt.value : lastSyncedAt;
      if (lastAt) {
        console.log(`Last synced event in Turso: ${lastAt}`);
        // Optimization: only send events newer than last known (still using ON CONFLICT for safety)
        const newEvents = eventLog.filter((e: any) => e.at >= lastAt);
        syncStatements = [];
        for (const event of newEvents) {
          const eid = eventId(event);
          const cols = ['external_id', 'event_type', 'destination', 'process', 'data', 'created_at'];
          const values = [
            sqlText(eid),
            sqlText(event.event),
            sqlText(event.destination || null),
            sqlText(event.process || null),
            sqlText(event.data ? JSON.stringify(event.data) : null),
            sqlText(event.at),
          ];
          syncStatements.push(`INSERT INTO events (${cols.join(',')}) VALUES (${values.join(',')}) ON CONFLICT(external_id) DO NOTHING;`);
        }
      }
    }
  } catch (e) {
    console.warn('Could not check last synced event, syncing all:', (e as Error).message);
  }

  if (syncStatements.length === 0) {
    console.log('All events are already synced.');
    return;
  }

  console.log(`Syncing ${syncStatements.length} events (idempotent)...`);
  await client.executeMany(syncStatements);
  console.log('✅ Events sync complete.');
}

main().catch((e) => {
  console.error(`Error: ${(e as Error).message}`);
  process.exit(1);
});
