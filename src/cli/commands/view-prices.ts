import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';
import { validatePositiveInt } from '../../types/validation';
import { calculateLeave } from '../../utils/holiday-calculator';
import * as fs from 'fs';
import * as path from 'path';

// ── types (extracted from travel-update.ts) ──────────────────────────

interface DateRangeResult {
  depart_date: string;
  return_date: string;
  depart_day: string;
  return_day: string;
  outbound: {
    nonstop_cheapest_usd: number | null;
    nonstop_cheapest_airline: string | null;
    nonstop_cheapest_time: string | null;
    flights: Array<{
      airline: string;
      depart: string;
      arrive: string;
      duration: string;
      nonstop: boolean;
      price_per_person_usd: number;
      total_usd: number;
    }>;
  };
  inbound: {
    nonstop_cheapest_usd: number | null;
    nonstop_cheapest_airline: string | null;
    nonstop_cheapest_time: string | null;
    flights: Array<{
      airline: string;
      depart: string;
      arrive: string;
      duration: string;
      nonstop: boolean;
      price_per_person_usd: number;
      total_usd: number;
    }>;
  };
  combined_cheapest_usd: number | null;
  combined_cheapest_twd: number | null;
}

interface DateRangeData {
  scraped_at: string;
  params: {
    depart_start: string;
    depart_end: string;
    origin: string;
    dest: string;
    duration: number;
    pax: number;
  };
  results: DateRangeResult[];
}

interface ViewPricesOptions {
  hotelPerNight?: number;
  nights?: number;
  packagePrice?: number;
  pax: number;
  json: boolean;
}

// ── helpers (extracted from travel-update.ts) ────────────────────────

function autoDetectHotelPrice(): number | null {
  const dataDir = path.join(process.cwd(), 'data');
  if (!fs.existsSync(dataDir)) return null;

  const bookingFiles = fs.readdirSync(dataDir)
    .filter(f => f.startsWith('booking-') && f.endsWith('.json'))
    .sort()
    .reverse(); // newest first by name

  for (const file of bookingFiles) {
    try {
      const content = fs.readFileSync(path.join(dataDir, file), 'utf-8');
      const data = JSON.parse(content);
      const extracted = data.extracted || {};
      if (extracted.hotels && Array.isArray(extracted.hotels) && extracted.hotels.length > 0) {
        // Find cheapest hotel per-night price
        let cheapest: number | null = null;
        for (const hotel of extracted.hotels) {
          const price = hotel.price_per_night || hotel.price;
          if (typeof price === 'number' && (cheapest === null || price < cheapest)) {
            cheapest = price;
          }
        }
        if (cheapest !== null) return cheapest;
      }
      // Fallback: look for price patterns in raw_text
      const rawText = String(data.raw_text || '');
      const priceMatches = rawText.match(/TWD\s+([\d,]+)/g);
      if (priceMatches && priceMatches.length > 0) {
        const prices = priceMatches
          .map(m => parseInt(m.replace(/TWD\s+/, '').replace(/,/g, ''), 10))
          .filter(p => p > 1000 && p < 50000)
          .sort((a, b) => a - b);
        if (prices.length > 0) return prices[0];
      }
    } catch {
      // skip
    }
  }
  return null;
}

function showPriceComparison(flightsPath: string, opts: ViewPricesOptions): void {
  const resolvedPath = path.isAbsolute(flightsPath) ? flightsPath : path.join(process.cwd(), flightsPath);
  if (!fs.existsSync(resolvedPath)) {
    console.error(`Error: Flight data file not found: ${resolvedPath}`);
    process.exit(1);
  }

  let data: DateRangeData;
  try {
    data = JSON.parse(fs.readFileSync(resolvedPath, 'utf-8'));
  } catch (e) {
    console.error(`Error: Failed to parse flight data: ${(e as Error).message}`);
    process.exit(1);
  }

  if (!data.results || data.results.length === 0) {
    console.error('Error: No flight results found in data file.');
    process.exit(1);
  }

  const duration = data.params.duration;
  const pax = opts.pax || data.params.pax || 2;
  const hotelNights = opts.nights ?? (duration - 1);
  const usdToTwd = 32;

  // Auto-detect or use provided hotel price
  let hotelPerNight = opts.hotelPerNight;
  let hotelSource = 'manual';
  if (hotelPerNight === undefined) {
    const detected = autoDetectHotelPrice();
    if (detected !== null) {
      hotelPerNight = detected;
      hotelSource = 'auto (booking-*.json)';
    }
  }

  // Build comparison rows
  const rows: Array<{
    departDate: string;
    departDay: string;
    returnDate: string;
    returnDay: string;
    outAirline: string;
    outTime: string;
    outPrice: number;
    inAirline: string;
    inTime: string;
    inPrice: number;
    flightTotal: number;
    hotelTotal: number | null;
    separateTotal: number | null;
    packagePrice: number | null;
    diff: number | null;
    leaveDays: number;
  }> = [];

  for (const r of data.results) {
    const outPrice = r.outbound.nonstop_cheapest_usd;
    const inPrice = r.inbound.nonstop_cheapest_usd;
    if (outPrice === null || inPrice === null) continue;

    const flightTotalTwd = Math.round((outPrice + inPrice) * usdToTwd);
    const hotelTotalTwd = hotelPerNight !== undefined ? hotelPerNight * hotelNights : null;
    const separateTotal = hotelTotalTwd !== null ? flightTotalTwd + hotelTotalTwd : null;
    const diff = separateTotal !== null && opts.packagePrice !== undefined
      ? opts.packagePrice - separateTotal
      : null;

    // Calculate leave days
    const leavePlan = calculateLeave({
      startDate: r.depart_date,
      endDate: r.return_date,
      market: 'tw',
    });

    rows.push({
      departDate: r.depart_date,
      departDay: r.depart_day,
      returnDate: r.return_date,
      returnDay: r.return_day,
      outAirline: r.outbound.nonstop_cheapest_airline || '?',
      outTime: r.outbound.nonstop_cheapest_time || '?',
      outPrice,
      inAirline: r.inbound.nonstop_cheapest_airline || '?',
      inTime: r.inbound.nonstop_cheapest_time || '?',
      inPrice,
      flightTotal: flightTotalTwd,
      hotelTotal: hotelTotalTwd,
      separateTotal,
      packagePrice: opts.packagePrice ?? null,
      diff,
      leaveDays: leavePlan.leaveDaysNeeded,
    });
  }

  if (rows.length === 0) {
    console.error('Error: No valid flight data with nonstop prices found.');
    process.exit(1);
  }

  // Sort by separate total (cheapest first)
  rows.sort((a, b) => (a.separateTotal ?? Infinity) - (b.separateTotal ?? Infinity));

  if (opts.json) {
    console.log(JSON.stringify({ params: data.params, pax, hotelPerNight, hotelNights, hotelSource, rows }, null, 2));
    return;
  }

  // Print header
  console.log(`\n  PACKAGE vs SEPARATE BOOKING COMPARISON`);
  console.log(`  Route: ${data.params.origin.toUpperCase()} → ${data.params.dest.toUpperCase()} | ${duration} days | ${pax} pax`);
  console.log(`  Flight data: ${path.basename(flightsPath)} (scraped: ${data.scraped_at.slice(0, 16)})`);
  if (hotelPerNight !== undefined) {
    console.log(`  Hotel: TWD ${hotelPerNight.toLocaleString()}/night x ${hotelNights} nights = TWD ${(hotelPerNight * hotelNights).toLocaleString()} (${hotelSource})`);
  }
  if (opts.packagePrice !== undefined) {
    console.log(`  Package baseline: TWD ${opts.packagePrice.toLocaleString()} (${pax} pax)`);
  }
  console.log('');

  // Print comparison table
  console.log('-----------------------------------------------------------------------------------------------');
  console.log('  Depart       Return       Outbound              Inbound               Flight     Hotel      Total      Leave');
  console.log('-----------------------------------------------------------------------------------------------');

  for (let i = 0; i < rows.length; i++) {
    const r = rows[i];
    const marker = i === 0 ? ' *' : '';
    const departCol = `${r.departDate.slice(5)} (${r.departDay})`.padEnd(10);
    const returnCol = `${r.returnDate.slice(5)} (${r.returnDay})`.padEnd(10);

    const outStr = `US$${r.outPrice} ${r.outAirline.slice(0, 8)}`.padEnd(20);
    const inStr = `US$${r.inPrice} ${r.inAirline.slice(0, 8)}`.padEnd(20);
    const flightStr = `TWD ${r.flightTotal.toLocaleString()}`.padEnd(10);
    const hotelStr = r.hotelTotal !== null ? `TWD ${r.hotelTotal.toLocaleString()}`.padEnd(10) : '-'.padEnd(10);
    const totalStr = r.separateTotal !== null ? `TWD ${r.separateTotal.toLocaleString()}`.padEnd(10) : '-'.padEnd(10);
    const leaveStr = `${r.leaveDays}d${marker}`.padEnd(6);

    console.log(`  ${departCol} ${returnCol} ${outStr} ${inStr} ${flightStr} ${hotelStr} ${totalStr} ${leaveStr}`);
  }

  console.log('-----------------------------------------------------------------------------------------------');

  // Package comparison
  if (opts.packagePrice !== undefined) {
    console.log('\n  Package vs Separate:');
    for (const r of rows) {
      if (r.separateTotal === null || r.diff === null) continue;
      const diffStr = r.diff > 0
        ? `Package costs TWD ${r.diff.toLocaleString()} more (+${Math.round(r.diff / r.separateTotal * 100)}%)`
        : r.diff < 0
          ? `Separate costs TWD ${Math.abs(r.diff).toLocaleString()} more (+${Math.round(Math.abs(r.diff) / opts.packagePrice * 100)}%)`
          : 'Same price';
      console.log(`    ${r.departDate.slice(5)} (${r.departDay}): ${diffStr}`);
    }
  }

  // Best value summary
  const best = rows[0];
  console.log(`\n  * Cheapest: ${best.departDate} (${best.departDay}) departure`);
  console.log(`    Outbound: ${best.outAirline} ${best.outTime} - US$${best.outPrice}`);
  console.log(`    Inbound:  ${best.inAirline} ${best.inTime} - US$${best.inPrice}`);
  if (best.separateTotal !== null) {
    console.log(`    Separate total: TWD ${best.separateTotal.toLocaleString()} (${pax} pax)`);
  }
  console.log(`    Leave needed: ${best.leaveDays} days`);

  // LCC baggage warning
  console.log('\n  Warning: LCC fares do not include checked baggage (~TWD 1,500-2,000/person round-trip)');
  console.log('');
}

// ── view-prices ──────────────────────────────────────────────────────

const viewPricesCommand: CommandHandler = {
  names: ['view-prices'],
  description: 'Compare package vs separate booking (flight + hotel) prices.',
  usage: 'view-prices --flights <file> [--hotel-per-night N] [--nights N] [--package N] [--pax N] [--json]',
  async execute(ctx: CliContext): Promise<void> {
    const { args } = ctx;
    const flightsOpt = args.optionValue('--flights');
    const paxOpt = args.optionValue('--pax');
    const hotelPerNightOpt = args.optionValue('--hotel-per-night');
    const nightsOpt = args.optionValue('--nights');
    const packageOpt = args.optionValue('--package');
    const jsonOpt = args.hasFlag('--json');

    if (!flightsOpt) {
      console.error('Error: view-prices requires --flights <file>');
      console.error('Example: view-prices --flights scrapes/date-range-prices.json --hotel-per-night 3000 --nights 4');
      process.exit(1);
    }

    let pax = 2;
    if (paxOpt) {
      const paxResult = validatePositiveInt(paxOpt, '--pax');
      if (!paxResult.ok) {
        console.error(`Error: ${paxResult.error}`);
        process.exit(1);
      }
      pax = paxResult.value;
    }

    const hotelPerNight = hotelPerNightOpt ? parseInt(hotelPerNightOpt, 10) : undefined;
    const nights = nightsOpt ? parseInt(nightsOpt, 10) : undefined;
    const packagePrice = packageOpt ? parseInt(packageOpt, 10) : undefined;

    showPriceComparison(flightsOpt, { hotelPerNight, nights, packagePrice, pax, json: jsonOpt });
  },
};

registerCommand(viewPricesCommand);
