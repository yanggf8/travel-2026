import * as fs from 'fs';
import * as path from 'path';

const KEY_MAP: Record<string, string> = {
  p2_destination: 'process_2_destination',
  p3_flights: 'process_3_transportation',
  p3_4_packages: 'process_3_4_packages',
  p4_hotels: 'process_4_accommodation',
  p5_itinerary: 'process_5_daily_itinerary',
};

const KNOWN_NEW_KEYS = new Set([
  'process_1_date_anchor',
  'process_2_destination',
  'process_3_transportation',
  'process_3_4_packages',
  'process_4_accommodation',
  'process_5_daily_itinerary',
]);

function stableStringify(value: unknown): string {
  if (value === null || typeof value !== 'object') {
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) {
    return `[${value.map(stableStringify).join(',')}]`;
  }
  const obj = value as Record<string, unknown>;
  const keys = Object.keys(obj).sort();
  const entries = keys.map(key => `${JSON.stringify(key)}:${stableStringify(obj[key])}`);
  return `{${entries.join(',')}}`;
}

function eventSignature(event: Record<string, unknown>): string {
  const data = event.data ? stableStringify(event.data) : '';
  return `${event.at || ''}|${event.event || ''}|${event.destination || ''}|${event.process || ''}|${data}`;
}

function getLatestEventTimestamp(events: Array<Record<string, unknown>> | undefined): Date | null {
  if (!events || events.length === 0) return null;
  const validTimestamps = events
    .map(e => (typeof e.at === 'string' ? new Date(e.at).getTime() : NaN))
    .filter(t => Number.isFinite(t));
  if (validTimestamps.length === 0) return null;
  return new Date(Math.max(...validTimestamps));
}

const statePath = 'data/state.json';
const state = JSON.parse(fs.readFileSync(statePath, 'utf-8'));
const warnings: string[] = [];

for (const [destSlug, dest] of Object.entries(state.destinations ?? {})) {
  const destObj = dest as { processes?: Record<string, Record<string, unknown>> };
  if (!destObj.processes) {
    warnings.push(`${destSlug}: no processes object`);
    continue;
  }

  const processes = destObj.processes;

  for (const key of Object.keys(processes)) {
    if (!KEY_MAP[key] && !KNOWN_NEW_KEYS.has(key)) {
      warnings.push(`${destSlug}: unmapped key '${key}'`);
    }
  }

  for (const [oldKey, newKey] of Object.entries(KEY_MAP)) {
    if (oldKey === newKey) continue;
    const legacy = processes[oldKey];
    const current = processes[newKey];

    if (!legacy) continue;

    if (!current) {
      processes[newKey] = legacy;
    } else {
      const legacyEvents = (legacy.events as Array<Record<string, unknown>> | undefined) ?? [];
      const currentEvents = (current.events as Array<Record<string, unknown>> | undefined) ?? [];
      const mergedEvents = [...legacyEvents, ...currentEvents].sort((a, b) => {
        const atA = typeof a.at === 'string' ? new Date(a.at).getTime() : NaN;
        const atB = typeof b.at === 'string' ? new Date(b.at).getTime() : NaN;
        if (!Number.isFinite(atA) && !Number.isFinite(atB)) return 0;
        if (!Number.isFinite(atA)) return 1;
        if (!Number.isFinite(atB)) return -1;
        return atA - atB;
      });

      const seen = new Set<string>();
      const uniqueEvents = mergedEvents.filter(event => {
        const signature = eventSignature(event);
        if (seen.has(signature)) return false;
        seen.add(signature);
        return true;
      });

      const legacyLatest = getLatestEventTimestamp(legacyEvents);
      const currentLatest = getLatestEventTimestamp(currentEvents);

      let resolvedState: unknown;
      if (!legacyLatest && !currentLatest) {
        resolvedState = current.state ?? legacy.state ?? 'pending';
      } else if (!legacyLatest) {
        resolvedState = current.state ?? legacy.state ?? 'pending';
      } else if (!currentLatest) {
        resolvedState = legacy.state ?? current.state ?? 'pending';
      } else {
        resolvedState = legacyLatest > currentLatest ? legacy.state : current.state;
      }

      processes[newKey] = {
        ...legacy,
        ...current,
        state: resolvedState,
        events: uniqueEvents,
      };
    }

    delete processes[oldKey];
  }
}

if (warnings.length > 0) {
  console.log('=== WARNINGS ===');
  warnings.forEach(warning => console.log(`  ${warning}`));
}

const tempPath = path.join(path.dirname(statePath), `.state.tmp.${process.pid}`);
fs.writeFileSync(tempPath, JSON.stringify(state, null, 2));
fs.renameSync(tempPath, statePath);

console.log('Migration complete');
