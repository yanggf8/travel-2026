import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { TursoPipelineClient, tursoText, tursoInt } from './turso-pipeline';

const ROOT = path.resolve(__dirname, '..');
const IMPORTED_AT = new Date().toISOString();

function readJson<T>(relPath: string): T {
  const fullPath = path.join(ROOT, relPath);
  return JSON.parse(fs.readFileSync(fullPath, 'utf-8')) as T;
}

function exists(relPath: string): boolean {
  return fs.existsSync(path.join(ROOT, relPath));
}

function stableId(parts: string[]): string {
  return crypto.createHash('sha1').update(parts.join('|')).digest('hex').slice(0, 16);
}

function firstJsonPayload(raw: string): unknown | null {
  const firstObject = raw.indexOf('{');
  const firstArray = raw.indexOf('[');
  const idxs = [firstObject, firstArray].filter((i) => i >= 0);
  if (idxs.length === 0) return null;
  const start = Math.min(...idxs);
  try {
    return JSON.parse(raw.slice(start));
  } catch {
    return null;
  }
}

function jsonLines(raw: string): unknown[] {
  const out: unknown[] = [];
  for (const line of raw.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed.startsWith('{') && !trimmed.startsWith('[')) continue;
    try {
      out.push(JSON.parse(trimmed));
    } catch {
      // Keep going; command-output banners are expected in these checkpoint files.
    }
  }
  return out;
}

function parseRawJson(value: unknown): Record<string, unknown> {
  if (typeof value !== 'string' || !value.trim()) return {};
  try {
    const parsed = JSON.parse(value);
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed)
      ? parsed as Record<string, unknown>
      : {};
  } catch {
    return {};
  }
}

function asString(value: unknown): string | null {
  if (value === null || value === undefined) return null;
  return String(value);
}

function asNumber(value: unknown): number | null {
  if (value === null || value === undefined || value === '') return null;
  const n = Number(value);
  return Number.isFinite(n) ? n : null;
}

async function backfillHotelAreas(client: TursoPipelineClient): Promise<number> {
  if (!exists('data/hotel-areas.json')) return 0;
  const areas = readJson<Record<string, Record<string, string[]>>>('data/hotel-areas.json');
  const stmts: Array<{ sql: string; args: any[] }> = [];
  for (const [region, regionAreas] of Object.entries(areas)) {
    for (const [areaType, keywords] of Object.entries(regionAreas)) {
      stmts.push({
        sql: `INSERT OR REPLACE INTO hotel_areas
          (region, area_type, keywords_json, source_url, fetched_at, confidence)
          VALUES (?, ?, ?, ?, ?, ?);`,
        args: [
          tursoText(region),
          tursoText(areaType),
          tursoText(JSON.stringify(keywords)),
          tursoText('legacy:data/hotel-areas.json'),
          tursoText(IMPORTED_AT),
          tursoText('legacy_backfill'),
        ],
      });
    }
  }
  await client.executeManyParams(stmts);
  return stmts.length;
}

async function backfillTransport(client: TursoPipelineClient): Promise<{ routes: number; hubs: number }> {
  if (!exists('data/transport-routes.json')) return { routes: 0, hubs: 0 };
  const transport = readJson<Record<string, {
    routes: Record<string, { time_min: number; cost_jpy: number; method: string }>;
    hubs: Record<string, { type: string; area: string }>;
  }>>('data/transport-routes.json');

  const routeStmts: Array<{ sql: string; args: any[] }> = [];
  const hubStmts: Array<{ sql: string; args: any[] }> = [];
  for (const [region, regionData] of Object.entries(transport)) {
    for (const [routeKey, route] of Object.entries(regionData.routes || {})) {
      const [fromHub, toHub] = routeKey.split('-');
      routeStmts.push({
        sql: `INSERT OR REPLACE INTO transport_routes
          (region, route_key, from_hub, to_hub, time_min, cost_jpy, method, source_url, fetched_at, confidence)
          VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?);`,
        args: [
          tursoText(region),
          tursoText(routeKey),
          tursoText(fromHub || null),
          tursoText(toHub || null),
          tursoInt(route.time_min),
          tursoInt(route.cost_jpy),
          tursoText(route.method),
          tursoText('legacy:data/transport-routes.json'),
          tursoText(IMPORTED_AT),
          tursoText('legacy_backfill'),
        ],
      });
    }
    for (const [hubId, hub] of Object.entries(regionData.hubs || {})) {
      hubStmts.push({
        sql: `INSERT OR REPLACE INTO transport_hubs
          (region, hub_id, hub_type, area, source_url, fetched_at, confidence)
          VALUES (?, ?, ?, ?, ?, ?, ?);`,
        args: [
          tursoText(region),
          tursoText(hubId),
          tursoText(hub.type),
          tursoText(hub.area),
          tursoText('legacy:data/transport-routes.json'),
          tursoText(IMPORTED_AT),
          tursoText('legacy_backfill'),
        ],
      });
    }
  }
  await client.executeManyParams([...routeStmts, ...hubStmts]);
  return { routes: routeStmts.length, hubs: hubStmts.length };
}

async function backfillDestinationReferences(client: TursoPipelineClient): Promise<number> {
  const dir = path.join(ROOT, 'src', 'skills', 'travel-shared', 'references', 'destinations');
  const slugByRefId: Record<string, string> = {
    tokyo: 'tokyo_2026',
    nagoya: 'nagoya_2026',
    osaka: 'osaka_2026',
    osaka_kyoto: 'osaka_kyoto_2026',
  };
  const stmts: Array<{ sql: string; args: any[] }> = [];
  for (const file of fs.readdirSync(dir).filter((f) => f.endsWith('.json'))) {
    const refId = path.basename(file, '.json');
    const slug = slugByRefId[refId];
    if (!slug) continue;
    const payload = fs.readFileSync(path.join(dir, file), 'utf-8');
    stmts.push({
      sql: `INSERT OR REPLACE INTO destination_references
        (slug, ref_id, payload_json, source_url, fetched_at, confidence)
        VALUES (?, ?, ?, ?, ?, ?);`,
      args: [
        tursoText(slug),
        tursoText(refId),
        tursoText(payload),
        tursoText(`legacy:src/skills/travel-shared/references/destinations/${file}`),
        tursoText(IMPORTED_AT),
        tursoText('legacy_backfill'),
      ],
    });
  }
  if (stmts.length > 0) await client.executeManyParams(stmts);
  return stmts.length;
}

async function upsertOkinawaDestination(client: TursoPipelineClient): Promise<void> {
  await client.executeParams(
    `INSERT OR IGNORE INTO destination_config
      (slug, display_name, ref_id, ref_path, timezone, currency, markets_json, primary_airports_json, language, origin, lat, lon)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?);`,
    [
      tursoText('okinawa_2026'),
      tursoText('Okinawa'),
      tursoText('okinawa'),
      tursoText(''),
      tursoText('Asia/Tokyo'),
      tursoText('JPY'),
      tursoText('["TW","JP"]'),
      tursoText('["OKA"]'),
      tursoText('ja'),
      tursoText('taiwan'),
      { type: 'float', value: 26.2124 },
      { type: 'float', value: 127.6792 },
    ],
  );
}

async function upsertStage0Run(client: TursoPipelineClient, payload: any): Promise<number> {
  if (!payload || typeof payload !== 'object' || !payload.run_id) return 0;
  let count = 0;
  await client.executeParams(
    `INSERT OR REPLACE INTO stage0_research_runs
      (run_id, origin_code, pax, window_start, window_end, currency, exchange_rate_usd_twd, status, created_at, updated_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?);`,
    [
      tursoText(payload.run_id),
      tursoText(payload.origin_code || 'TPE'),
      tursoInt(payload.pax || 2),
      tursoText(payload.window_start),
      tursoText(payload.window_end),
      tursoText(payload.currency || 'TWD'),
      { type: 'float', value: Number(payload.exchange_rate_usd_twd || 32) },
      tursoText(payload.status || 'imported'),
      tursoText(payload.created_at || IMPORTED_AT),
      tursoText(payload.updated_at || IMPORTED_AT),
    ],
  );
  count++;

  const stmts: Array<{ sql: string; args: any[] }> = [];
  for (const d of payload.destinations || []) {
    stmts.push({
      sql: `INSERT OR REPLACE INTO stage0_research_destinations
        (run_id, dest_code, dest_label, sort_order) VALUES (?, ?, ?, ?);`,
      args: [tursoText(payload.run_id), tursoText(d.dest_code), tursoText(d.dest_label), tursoInt(d.sort_order || 0)],
    });
  }
  for (const d of payload.durations || []) {
    stmts.push({
      sql: `INSERT OR REPLACE INTO stage0_research_durations
        (run_id, nights, duration_days) VALUES (?, ?, ?);`,
      args: [tursoText(payload.run_id), tursoInt(d.nights), tursoInt(d.duration_days)],
    });
  }
  for (const s of payload.shaping || []) {
    stmts.push({
      sql: `INSERT OR IGNORE INTO stage0_research_shaping
        (run_id, aspect, role, kind, value_text, value_date, value_integer, notes, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?);`,
      args: [
        tursoText(payload.run_id),
        tursoText(s.aspect),
        tursoText(s.role),
        tursoText(s.kind),
        tursoText(s.value_text ?? null),
        tursoText(s.value_date ?? null),
        tursoInt(s.value_integer ?? null),
        tursoText(s.notes ?? null),
        tursoText(s.created_at || IMPORTED_AT),
      ],
    });
  }
  for (const a of payload.attempts || []) {
    stmts.push({
      sql: `INSERT OR REPLACE INTO stage0_scrape_attempts
        (run_id, dest_code, nights, status, candidate_count, error, attempted_at)
        VALUES (?, ?, ?, ?, ?, ?, ?);`,
      args: [
        tursoText(payload.run_id),
        tursoText(a.dest_code),
        tursoInt(a.nights),
        tursoText(a.status),
        tursoInt(a.candidate_count),
        tursoText(a.error ?? null),
        tursoText(a.attempted_at || IMPORTED_AT),
      ],
    });
  }
  if (stmts.length > 0) {
    await client.executeManyParams(stmts);
    count += stmts.length;
  }
  return count;
}

async function upsertTourOffers(client: TursoPipelineClient, offers: any[]): Promise<{ offers: number; selections: number }> {
  const offerStmts: Array<{ sql: string; args: any[] }> = [];
  const selectionStmts: Array<{ sql: string; args: any[] }> = [];

  for (const o of offers) {
    if (!o || typeof o !== 'object' || !o.run_id || !o.offer_id) continue;
    offerStmts.push({
      sql: `INSERT OR REPLACE INTO stage0_tour_group_offers
        (run_id, offer_id, source_id, dest_region, depart_date, return_date, nights, price_per_person_twd,
         title, url, scraped_at, hotel_name, hotel_star_rating, meals_included_count, departure_status,
         seats_available, min_group_size, group_size_cap, raw_json, parse_warnings_json, product_kind)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?);`,
      args: [
        tursoText(o.run_id),
        tursoText(o.offer_id),
        tursoText(o.source_id),
        tursoText(o.dest_region),
        tursoText(o.depart_date),
        tursoText(o.return_date),
        tursoInt(o.nights),
        tursoInt(o.price_per_person_twd),
        tursoText(o.title),
        tursoText(o.url),
        tursoText(o.scraped_at),
        tursoText(o.hotel_name ?? null),
        tursoInt(o.hotel_star_rating ?? null),
        tursoInt(o.meals_included_count ?? null),
        tursoText(o.departure_status ?? null),
        tursoInt(o.seats_available ?? null),
        tursoInt(o.min_group_size ?? null),
        tursoInt(o.group_size_cap ?? null),
        tursoText(o.raw_json ?? null),
        tursoText(o.parse_warnings_json ?? null),
        tursoText(o.product_kind || 'fit'),
      ],
    });

    const rawJson = parseRawJson(o.raw_json);
    if (rawJson.selected === true || rawJson.selected === 1 || rawJson.selected === 'true') {
      const selectionId = `${o.run_id}:${o.offer_id}`;
      selectionStmts.push({
        sql: `INSERT OR REPLACE INTO stage0_selected_offers
          (selection_id, run_id, destination_slug, source_id, source_offer_id, selected_depart_date,
           selected_return_date, nights, price_per_person_twd, price_total_twd, hotel_name,
           observed_by, observed_at, selected_by, selected_at, provenance_json, raw_json, imported_at)
          VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?);`,
        args: [
          tursoText(selectionId),
          tursoText(o.run_id),
          tursoText('okinawa_2026'),
          tursoText(o.source_id),
          tursoText(o.offer_id),
          tursoText(o.depart_date),
          tursoText(o.return_date),
          tursoInt(o.nights),
          tursoInt(o.price_per_person_twd),
          tursoInt(asNumber(rawJson.trip_total_2pax_twd) ?? (asNumber(o.price_per_person_twd) || 0) * 2),
          tursoText(o.hotel_name ?? asString(rawJson.hotel_full_name)),
          tursoText(asString(rawJson.observed_by)),
          tursoText(asString(rawJson.observed_at)),
          tursoText(asString(rawJson.selected_by)),
          tursoText(asString(rawJson.selected_at)),
          tursoText(JSON.stringify(rawJson)),
          tursoText(o.raw_json ?? null),
          tursoText(IMPORTED_AT),
        ],
      });
    }
  }

  await client.executeManyParams([...offerStmts, ...selectionStmts]);
  return { offers: offerStmts.length, selections: selectionStmts.length };
}

async function backfillResearch(client: TursoPipelineClient): Promise<{ artifacts: number; stage0Rows: number; offers: number; selections: number }> {
  const dir = path.join(ROOT, 'research', 'okinawa-2026');
  if (!fs.existsSync(dir)) return { artifacts: 0, stage0Rows: 0, offers: 0, selections: 0 };
  const files = fs.readdirSync(dir).filter((f) => f.endsWith('.json') || f.endsWith('.txt') || f === 'README.md');
  let artifacts = 0;
  let stage0Rows = 0;
  let offers = 0;
  let selections = 0;

  for (const file of files) {
    const fullPath = path.join(dir, file);
    const raw = fs.readFileSync(fullPath, 'utf-8');
    const payload = firstJsonPayload(raw);
    const lines = jsonLines(raw);
    const runId =
      (payload && typeof payload === 'object' && !Array.isArray(payload) && asString((payload as any).run_id)) ||
      (lines.find((x) => x && typeof x === 'object' && !Array.isArray(x) && (x as any).run_id) as any)?.run_id ||
      'stage0-20260525-093508';
    const artifactId = `okinawa_2026:${stableId([file, runId])}`;
    const artifactKind = file.includes('tour-group-offers')
      ? 'tour_group_offers'
      : file.includes('shaping')
        ? 'shaping'
        : file.includes('qualified')
          ? 'qualified_candidates'
          : file.includes('stage0-export')
            ? 'stage0_export'
            : 'checkpoint';

    await client.executeParams(
      `INSERT OR REPLACE INTO stage0_research_artifacts
        (artifact_id, run_id, destination_slug, artifact_kind, original_filename, payload_json, raw_text,
         observed_by, observed_at, imported_at)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?);`,
      [
        tursoText(artifactId),
        tursoText(runId),
        tursoText('okinawa_2026'),
        tursoText(artifactKind),
        tursoText(file),
        tursoText(payload ? JSON.stringify(payload) : null),
        tursoText(payload ? null : raw),
        tursoText('yang+claude'),
        tursoText('2026-05-27'),
        tursoText(IMPORTED_AT),
      ],
    );
    artifacts++;

    if (payload && typeof payload === 'object' && !Array.isArray(payload) && (payload as any).run_id) {
      stage0Rows += await upsertStage0Run(client, payload);
    }

    const possibleOffers = Array.isArray(payload)
      ? payload
      : lines.filter((x) => x && typeof x === 'object' && !Array.isArray(x) && (x as any).offer_id);
    if (possibleOffers.length > 0) {
      const res = await upsertTourOffers(client, possibleOffers);
      offers += res.offers;
      selections += res.selections;
    }

    const shapingRows = lines.filter((x) => x && typeof x === 'object' && !Array.isArray(x) && (x as any).aspect && (x as any).role);
    if (shapingRows.length > 0) {
      await client.executeManyParams(shapingRows.map((s: any) => ({
        sql: `INSERT OR IGNORE INTO stage0_research_shaping
          (run_id, aspect, role, kind, value_text, value_date, value_integer, notes, created_at)
          VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?);`,
        args: [
          tursoText(s.run_id || runId),
          tursoText(s.aspect),
          tursoText(s.role),
          tursoText(s.kind),
          tursoText(s.value_text ?? null),
          tursoText(s.value_date ?? null),
          tursoInt(s.value_integer ?? null),
          tursoText(s.notes ?? null),
          tursoText(s.created_at || IMPORTED_AT),
        ],
      })));
      stage0Rows += shapingRows.length;
    }
  }

  return { artifacts, stage0Rows, offers, selections };
}

async function main(): Promise<void> {
  const client = new TursoPipelineClient();
  await upsertOkinawaDestination(client);
  const hotelAreas = await backfillHotelAreas(client);
  const transport = await backfillTransport(client);
  const destinationRefs = await backfillDestinationReferences(client);
  const research = await backfillResearch(client);

  console.log(`hotel_areas rows: ${hotelAreas}`);
  console.log(`transport_routes rows: ${transport.routes}`);
  console.log(`transport_hubs rows: ${transport.hubs}`);
  console.log(`destination_references rows: ${destinationRefs}`);
  console.log(`stage0 artifacts: ${research.artifacts}`);
  console.log(`stage0 rows touched: ${research.stage0Rows}`);
  console.log(`stage0 offers imported: ${research.offers}`);
  console.log(`stage0 selections imported: ${research.selections}`);
}

main().catch((err) => {
  console.error(err instanceof Error ? err.message : String(err));
  process.exit(1);
});
