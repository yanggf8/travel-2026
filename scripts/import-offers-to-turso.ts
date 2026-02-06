import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { TursoPipelineClient } from './turso-pipeline';

type OfferRow = {
  id: string;
  source_id: string;
  type: 'package' | 'flight' | 'hotel';
  name: string | null;
  price_per_person: number | null;
  currency: string | null;
  region: string | null;
  destination: string | null;
  departure_date: string | null;
  return_date: string | null;
  nights: number | null;
  availability: string | null;
  hotel_name: string | null;
  airline: string | null;
  raw_data: string;
  scraped_at: string | null;
};

function usage(): string {
  return [
    'Import scraped JSON files into Turso offers table (query layer only).',
    '',
    'Usage:',
    '  npm run db:import:turso -- --dir data --destination tokyo_2026 --region tokyo',
    '  npm run db:import:turso -- --files data/besttour-kansai-feb24.json,data/liontravel-osaka-group.json',
    '',
    'Options:',
    '  --dir <path>             Directory to scan (default: data)',
    '  --files <csv>            Comma-separated JSON file paths (overrides --dir)',
    '  --destination <slug>     Destination slug (e.g. tokyo_2026, osaka_kyoto_2026). If omitted, inferred from filename.',
    '  --region <name>          Region label (e.g. kansai, tokyo). If omitted, inferred from filename.',
    '  --dry-run                Parse/normalize only; do not write to Turso',
    '  --events                 Also append audit rows into Turso events table',
    '  --endpoint <url>         Override Turso pipeline endpoint (or set TURSO_HTTP_ENDPOINT)',
    '',
    'Env:',
    '  TURSO_TOKEN must be set (or in .env).',
    '',
  ].join('\n');
}

function optionValue(argv: string[], name: string): string | undefined {
  const i = argv.indexOf(name);
  if (i === -1) return undefined;
  return argv[i + 1];
}

function hasFlag(argv: string[], name: string): boolean {
  return argv.includes(name);
}

function stableHash8(input: string): string {
  return crypto.createHash('sha1').update(input).digest('hex').slice(0, 8);
}

function sqlEscapeText(value: string): string {
  return value.replace(/'/g, "''");
}

function sqlText(value: string | null): string {
  if (value === null) return 'NULL';
  return `'${sqlEscapeText(value)}'`;
}

function sqlInt(value: number | null): string {
  if (value === null || !Number.isFinite(value)) return 'NULL';
  return `${Math.trunc(value)}`;
}

function normalizeIsoDate(s: unknown): string | null {
  if (typeof s !== 'string') return null;
  const v = s.trim();
  if (!v) return null;
  // Accept YYYY-MM-DD
  if (/^\d{4}-\d{2}-\d{2}$/.test(v)) return v;
  // Convert YYYY/MM/DD
  const m = v.match(/^(\d{4})\/(\d{1,2})\/(\d{1,2})$/);
  if (m) {
    const mm = m[2].padStart(2, '0');
    const dd = m[3].padStart(2, '0');
    return `${m[1]}-${mm}-${dd}`;
  }
  return null;
}

function inferSourceIdFromUrl(url: string): string {
  const u = url.toLowerCase();
  if (u.includes('besttour.com.tw')) return 'besttour';
  if (u.includes('liontravel.com')) return 'liontravel';
  if (u.includes('lifetour.com.tw')) return 'lifetour';
  if (u.includes('settour.com.tw')) return 'settour';
  if (u.includes('eztravel')) return 'eztravel';
  if (u.includes('tigerair')) return 'tigerair';
  if (u.includes('agoda')) return 'agoda';
  if (u.includes('booking.com')) return 'booking';
  return 'unknown';
}

function inferSourceIdFromFilename(filename: string): string {
  const f = filename.toLowerCase();
  if (f.includes('besttour')) return 'besttour';
  if (f.includes('liontravel')) return 'liontravel';
  if (f.includes('lifetour')) return 'lifetour';
  if (f.includes('settour')) return 'settour';
  if (f.includes('eztravel')) return 'eztravel';
  if (f.includes('tigerair')) return 'tigerair';
  if (f.includes('agoda')) return 'agoda';
  if (f.includes('booking')) return 'booking';
  return 'unknown';
}

function inferDestinationFromFilename(filename: string): string | null {
  const f = filename.toLowerCase();
  if (f.includes('tokyo') || f.includes('tyo')) return 'tokyo_2026';
  if (f.includes('osaka_kyoto')) return 'osaka_kyoto_2026';
  if (f.includes('osaka') || f.includes('kansai') || f.includes('kyoto') || f.includes('kix')) return 'osaka_2026';
  return null;
}

function inferRegionFromFilename(filename: string): string | null {
  const f = filename.toLowerCase();
  const candidates = ['tokyo', 'kansai', 'hokkaido', 'kyushu', 'okinawa', 'osaka', 'kyoto', 'nagoya'];
  for (const c of candidates) {
    if (f.includes(c)) return c;
  }
  return null;
}

function productCodeFromUrl(url: string): string | null {
  if (!url) return null;
  try {
    const u = new URL(url);
    const segs = u.pathname.split('/').filter(Boolean);
    const last = segs.at(-1);
    if (!last) return null;
    const cleaned = last.replace(/\.html?$/i, '');
    return cleaned || null;
  } catch {
    // Not a valid URL; fall back to simple split.
    const base = url.split('?')[0] || '';
    const parts = base.split('/').filter(Boolean);
    const last = parts.at(-1);
    return last || null;
  }
}

function offerId(sourceId: string, url: string, explicitProductCode?: string): string {
  const productCode = explicitProductCode || productCodeFromUrl(url) || stableHash8(url);
  return `${sourceId}_${productCode}`;
}

function parseRawScrapeAsOfferRow(
  fileName: string,
  data: Record<string, unknown>,
  destinationOverride: string | null,
  regionOverride: string | null
): OfferRow | null {
  const url = typeof data.url === 'string' ? data.url : '';
  const title = typeof data.title === 'string' ? data.title : null;
  const scrapedAt = typeof data.scraped_at === 'string' ? data.scraped_at : null;
  const extracted = (data.extracted && typeof data.extracted === 'object') ? (data.extracted as Record<string, unknown>) : {};

  const sourceIdFromData =
    (typeof data.source_id === 'string' && data.source_id) ||
    (typeof data.sourceId === 'string' && data.sourceId) ||
    '';
  const sourceId =
    sourceIdFromData ||
    (url ? inferSourceIdFromUrl(url) : 'unknown') ||
    inferSourceIdFromFilename(fileName);
  if (sourceId === 'unknown') return null;

  const id = offerId(sourceId, url);

  const currency =
    (extracted.price && typeof extracted.price === 'object' && typeof (extracted.price as any).currency === 'string')
      ? String((extracted.price as any).currency)
      : 'TWD';

  let pricePerPerson: number | null = null;
  const perPerson =
    extracted.price && typeof extracted.price === 'object'
      ? (extracted.price as any).per_person
      : null;
  if (typeof perPerson === 'number' && Number.isFinite(perPerson)) pricePerPerson = perPerson;

  const flight = extracted.flight && typeof extracted.flight === 'object' ? (extracted.flight as any) : null;
  const hotel = extracted.hotel && typeof extracted.hotel === 'object' ? (extracted.hotel as any) : null;

  let type: 'package' | 'flight' | 'hotel' = 'package';
  const hasFlight = Boolean(flight && (flight.outbound || flight.return));
  const hasHotel = Boolean(hotel && (hotel.name || hotel.names));
  if (hasFlight && !hasHotel) type = 'flight';
  if (hasHotel && !hasFlight) type = 'hotel';

  const airline = (flight?.outbound?.airline && typeof flight.outbound.airline === 'string') ? flight.outbound.airline : null;
  const hotelName = (hotel?.name && typeof hotel.name === 'string') ? hotel.name : (Array.isArray(hotel?.names) && hotel.names[0] ? String(hotel.names[0]) : null);

  const dates = extracted.dates && typeof extracted.dates === 'object' ? (extracted.dates as any) : null;
  const departureDate = normalizeIsoDate(dates?.departure_date);
  const returnDate = normalizeIsoDate(dates?.return_date);
  const nights =
    typeof dates?.duration_nights === 'number' && Number.isFinite(dates.duration_nights)
      ? dates.duration_nights
      : null;

  const destination = destinationOverride || inferDestinationFromFilename(fileName);
  const region = regionOverride || inferRegionFromFilename(fileName);

  return {
    id,
    source_id: sourceId,
    type,
    name: title,
    price_per_person: pricePerPerson,
    currency,
    region,
    destination,
    departure_date: departureDate,
    return_date: returnDate,
    nights,
    availability: null,
    hotel_name: hotelName,
    airline,
    raw_data: JSON.stringify(data),
    scraped_at: scrapedAt,
  };
}

function parseScraperOfferAsOfferRow(
  fileName: string,
  offer: Record<string, unknown>,
  destinationOverride: string | null,
  regionOverride: string | null
): OfferRow | null {
  const url = typeof offer.url === 'string' ? offer.url : '';
  const sourceId =
    (typeof offer.source_id === 'string' && offer.source_id) ||
    (typeof offer.sourceId === 'string' && offer.sourceId) ||
    inferSourceIdFromUrl(url) ||
    inferSourceIdFromFilename(fileName);
  if (!sourceId || sourceId === 'unknown') return null;

  const explicitId =
    (typeof offer.id === 'string' && offer.id) ||
    null;
  const id = explicitId || offerId(sourceId, url, (typeof offer.product_code === 'string' ? offer.product_code : undefined));

  const typeRaw = (typeof offer.type === 'string' ? offer.type : 'package').toLowerCase();
  const type: 'package' | 'flight' | 'hotel' =
    typeRaw === 'flight' ? 'flight' : typeRaw === 'hotel' ? 'hotel' : 'package';

  const name =
    (typeof offer.name === 'string' && offer.name) ||
    (typeof offer.title === 'string' && offer.title) ||
    null;

  const pricePerPerson =
    typeof offer.price_per_person === 'number'
      ? offer.price_per_person
      : typeof offer.pricePerPerson === 'number'
        ? offer.pricePerPerson
        : null;

  const currency =
    (typeof offer.currency === 'string' && offer.currency) ||
    null;

  const scrapedAt =
    (typeof offer.scraped_at === 'string' && offer.scraped_at) ||
    (typeof offer.scrapedAt === 'string' && offer.scrapedAt) ||
    null;

  const hotelName =
    (offer.hotel && typeof offer.hotel === 'object' && typeof (offer.hotel as any).name === 'string')
      ? String((offer.hotel as any).name)
      : null;

  const airline =
    (offer.flight && typeof offer.flight === 'object' && (offer.flight as any).outbound && typeof (offer.flight as any).outbound.airline === 'string')
      ? String((offer.flight as any).outbound.airline)
      : null;

  const destination = destinationOverride || inferDestinationFromFilename(fileName);
  const region = regionOverride || inferRegionFromFilename(fileName);

  return {
    id,
    source_id: sourceId,
    type,
    name,
    price_per_person: typeof pricePerPerson === 'number' && Number.isFinite(pricePerPerson) ? pricePerPerson : null,
    currency,
    region,
    destination,
    departure_date: null,
    return_date: null,
    nights: null,
    availability: (typeof offer.availability === 'string' ? offer.availability : null),
    hotel_name: hotelName,
    airline,
    raw_data: JSON.stringify(offer),
    scraped_at: scrapedAt,
  };
}

function toUpsertSql(row: OfferRow): string {
  const cols = [
    'id',
    'source_id',
    'type',
    'name',
    'price_per_person',
    'currency',
    'region',
    'destination',
    'departure_date',
    'return_date',
    'nights',
    'availability',
    'hotel_name',
    'airline',
    'raw_data',
    'scraped_at',
  ];
  const values = [
    sqlText(row.id),
    sqlText(row.source_id),
    sqlText(row.type),
    sqlText(row.name),
    sqlInt(row.price_per_person),
    sqlText(row.currency),
    sqlText(row.region),
    sqlText(row.destination),
    sqlText(row.departure_date),
    sqlText(row.return_date),
    sqlInt(row.nights),
    sqlText(row.availability),
    sqlText(row.hotel_name),
    sqlText(row.airline),
    sqlText(row.raw_data),
    sqlText(row.scraped_at),
  ];

  const updates = cols
    .filter((c) => c !== 'id')
    .map((c) => `${c}=excluded.${c}`)
    .join(', ');

  return `INSERT INTO offers (${cols.join(',')}) VALUES (${values.join(',')}) ON CONFLICT(id) DO UPDATE SET ${updates};`;
}

async function main(): Promise<void> {
  const argv = process.argv.slice(2);
  if (argv.includes('--help') || argv.includes('-h')) {
    console.log(usage());
    process.exit(0);
  }

  const dir = optionValue(argv, '--dir') || 'data';
  const filesCsv = optionValue(argv, '--files');
  const destinationOverride = optionValue(argv, '--destination') || null;
  const regionOverride = optionValue(argv, '--region') || null;
  const dryRun = hasFlag(argv, '--dry-run');
  const emitEvents = hasFlag(argv, '--events');
  const endpoint = optionValue(argv, '--endpoint');

  const client = new TursoPipelineClient({ ...(endpoint ? { endpoint } : {}) });
  client.loadEnv();

  const files = filesCsv
    ? filesCsv.split(',').map((s) => s.trim()).filter(Boolean)
    : fs
        .existsSync(dir)
        ? fs
            .readdirSync(dir)
            .filter((f) => f.endsWith('.json'))
            .filter((f) => !f.includes('schema') && !f.includes('travel-plan') && !f.includes('destinations') && !f.includes('ota-sources'))
            .map((f) => path.join(dir, f))
        : [];

  if (files.length === 0) {
    console.error(`No JSON files found (dir=${dir}).`);
    console.error('Tip: pass --files file1.json,file2.json');
    process.exit(1);
  }

  let parsedOffers = 0;
  let skipped = 0;
  const sqlStatements: string[] = [];

  for (const filePath of files) {
    const fileName = path.basename(filePath);
    let raw: unknown;
    try {
      raw = JSON.parse(fs.readFileSync(filePath, 'utf-8')) as unknown;
    } catch (e) {
      skipped++;
      continue;
    }

    const offers: OfferRow[] = [];

    if (raw && typeof raw === 'object' && !Array.isArray(raw)) {
      const obj = raw as Record<string, unknown>;
      const maybeOffers = obj.offers;
      if (Array.isArray(maybeOffers)) {
        for (const o of maybeOffers) {
          if (!o || typeof o !== 'object' || Array.isArray(o)) continue;
          const row = parseScraperOfferAsOfferRow(fileName, o as Record<string, unknown>, destinationOverride, regionOverride);
          if (row) offers.push(row);
        }
      } else {
        const row = parseRawScrapeAsOfferRow(fileName, obj, destinationOverride, regionOverride);
        if (row) offers.push(row);
      }
    }

    if (offers.length === 0) {
      skipped++;
      continue;
    }

    parsedOffers += offers.length;
    for (const row of offers) {
      sqlStatements.push(toUpsertSql(row));
      if (emitEvents) {
        const eventData = {
          file: fileName,
          offer_id: row.id,
          source_id: row.source_id,
          type: row.type,
          destination: row.destination,
          region: row.region,
          scraped_at: row.scraped_at,
        };
        sqlStatements.push(
          `INSERT INTO events (event_type, destination, process, data) VALUES ('offer_imported', ${sqlText(row.destination)}, 'turso_import', ${sqlText(JSON.stringify(eventData))});`
        );
      }
    }
  }

  console.log(`Files scanned: ${files.length}`);
  console.log(`Offers parsed: ${parsedOffers}`);
  console.log(`Files skipped: ${skipped}`);
  console.log(`SQL statements: ${sqlStatements.length}`);

  if (dryRun) {
    console.log('Dry run: no writes performed.');
    return;
  }

  await client.executeMany(sqlStatements, 25);
  console.log('✅ Import complete.');
}

main().catch((e) => {
  console.error(`Error: ${(e as Error).message}`);
  process.exit(1);
});
