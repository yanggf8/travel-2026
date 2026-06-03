import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';
import { validatePositiveInt } from '../../types/validation';
import { calculateLeave } from '../../utils/holiday-calculator';
import { queryOffers } from '../../services/turso-service';

interface ViewPricesOptions {
  hotelPerNight?: number;
  nights?: number;
  packagePrice?: number;
  pax: number;
  json: boolean;
  startDate: string;
  endDate: string;
  region?: string;
  destination?: string;
}

// ── helpers (extracted from travel-update.ts) ────────────────────────

async function autoDetectHotelPrice(opts: Pick<ViewPricesOptions, 'region' | 'destination' | 'startDate' | 'endDate'>): Promise<number | null> {
  const hotels = await queryOffers({
    region: opts.region,
    destination: opts.destination,
    start: opts.startDate,
    end: opts.endDate,
    type: 'hotel',
    limit: 50,
  });
  const prices = hotels
    .map((h) => h.price_per_person)
    .filter((p): p is number => typeof p === 'number' && Number.isFinite(p) && p > 0)
    .sort((a, b) => a - b);
  return prices[0] ?? null;
}

async function showPriceComparison(opts: ViewPricesOptions): Promise<void> {
  const flightOffers = await queryOffers({
    region: opts.region,
    destination: opts.destination,
    start: opts.startDate,
    end: opts.endDate,
    type: 'flight',
    limit: 500,
  });
  if (flightOffers.length === 0) {
    throw new Error('Missing Turso flight offers. Import scraper output with npm run db:import:turso before running view-prices.');
  }

  const duration = opts.nights ? opts.nights + 1 : 5;
  const pax = opts.pax || 2;
  const hotelNights = opts.nights ?? (duration - 1);
  const usdToTwd = 32;

  // Auto-detect or use provided hotel price
  let hotelPerNight = opts.hotelPerNight;
  let hotelSource = 'manual';
  if (hotelPerNight === undefined) {
    const detected = await autoDetectHotelPrice(opts);
    if (detected !== null) {
      hotelPerNight = detected;
      hotelSource = 'auto (Turso hotel offers)';
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

  for (const offer of flightOffers) {
    if (!offer.departure_date || offer.price_per_person === null) continue;
    const outPrice = offer.price_per_person;
    const inPrice = 0;
    const flightTotalTwd = offer.currency === 'USD'
      ? Math.round(outPrice * usdToTwd)
      : outPrice * pax;
    const hotelTotalTwd = hotelPerNight !== undefined ? hotelPerNight * hotelNights : null;
    const separateTotal = hotelTotalTwd !== null ? flightTotalTwd + hotelTotalTwd : null;
    const diff = separateTotal !== null && opts.packagePrice !== undefined
      ? opts.packagePrice - separateTotal
      : null;

    // Calculate leave days
    const returnDate = offer.return_date || offer.departure_date;
    const leavePlan = await calculateLeave({
      startDate: offer.departure_date,
      endDate: returnDate,
      market: 'tw',
    });

    rows.push({
      departDate: offer.departure_date,
      departDay: '',
      returnDate,
      returnDay: '',
      outAirline: offer.airline || offer.source_id || '?',
      outTime: '',
      outPrice,
      inAirline: '',
      inTime: '',
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
    console.log(JSON.stringify({
      params: {
        start: opts.startDate,
        end: opts.endDate,
        region: opts.region,
        destination: opts.destination,
      },
      pax,
      hotelPerNight,
      hotelNights,
      hotelSource,
      rows,
    }, null, 2));
    return;
  }

  // Print header
  console.log(`\n  PACKAGE vs SEPARATE BOOKING COMPARISON`);
  console.log(`  Scope: ${opts.destination || opts.region || 'all'} | ${duration} days | ${pax} pax`);
  console.log(`  Flight data: Turso offers (${opts.startDate} to ${opts.endDate})`);
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
  usage: 'view-prices --start YYYY-MM-DD --end YYYY-MM-DD [--region name|--destination slug] [--hotel-per-night N] [--nights N] [--package N] [--pax N] [--json]',
  async execute(ctx: CliContext): Promise<void> {
    const { args } = ctx;
    const startOpt = args.optionValue('--start');
    const endOpt = args.optionValue('--end');
    const regionOpt = args.optionValue('--region');
    const destinationOpt = args.optionValue('--destination');
    const paxOpt = args.optionValue('--pax');
    const hotelPerNightOpt = args.optionValue('--hotel-per-night');
    const nightsOpt = args.optionValue('--nights');
    const packageOpt = args.optionValue('--package');
    const jsonOpt = args.hasFlag('--json');

    if (!startOpt || !endOpt) {
      console.error('Error: view-prices requires --start YYYY-MM-DD and --end YYYY-MM-DD');
      console.error('Example: view-prices --start 2026-02-24 --end 2026-02-28 --region kansai --hotel-per-night 3000 --nights 4');
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

    await showPriceComparison({
      startDate: startOpt,
      endDate: endOpt,
      region: regionOpt,
      destination: destinationOpt,
      hotelPerNight,
      nights,
      packagePrice,
      pax,
      json: jsonOpt,
    });
  },
};

registerCommand(viewPricesCommand);
