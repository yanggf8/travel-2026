#!/usr/bin/env npx ts-node
/**
 * Compare Dates CLI
 *
 * Compares FIT packages vs separate bookings across multiple departure dates.
 * Automatically gathers flight data, hotel estimates, leave days, and baggage costs.
 *
 * Usage:
 *   npx ts-node src/cli/compare-dates.ts --start 2026-02-24 --end 2026-02-28 --nights 4
 *   npx ts-node src/cli/compare-dates.ts --start 2026-02-24 --end 2026-02-28 --nights 4 --hotel-per-night 3200
 *   npm run compare-dates -- --start 2026-02-24 --end 2026-02-28 --nights 4
 */

import { loadHolidayCalendar, calculateLeaveDays, LeaveDayResult, HolidayCalendar } from '../utils/leave-calculator';
import { EXCHANGE_RATES, DEFAULTS, DEFAULT_LCC_BAGGAGE_FEE } from '../config/constants';
import { addDays, getDayOfWeek } from '../utils/date-utils';
import { Result } from '../types';
import { queryOffers, TursoOfferResult } from '../services/turso-service';

// Types
interface FITPackage {
  file: string;
  departDate: string;
  returnDate: string;
  priceTotalTWD: number;
  pricePerPerson: number;
  airline: string;
  hotel: string;
  baggageIncluded: boolean;
  pax: number;
}

interface SeparateBooking {
  departDate: string;
  returnDate: string;
  outboundFlight: { airline: string; price: number; priceTWD: number; baggage: boolean } | null;
  returnFlight: { airline: string; price: number; priceTWD: number; baggage: boolean } | null;
  hotelTotalTWD: number;
  baggageCostTWD: number;
  totalTWD: number;
  pax: number;
}

interface DateComparison {
  departDate: string;
  returnDate: string;
  dayOfWeek: string;
  leaveDays: number;
  leaveDates: string[];
  fit: FITPackage | null;
  separate: SeparateBooking | null;
  fitVsSeparate: number | null; // positive = FIT is more expensive
}

interface CompareOptions {
  startDate: string;
  endDate: string;
  nights: number;
  hotelPerNight: number;
  market: string;
  region?: string;
  destination?: string;
  pax: number;
  baggageFeePerPersonPerDir: number;
}

const DAY_NAMES_ZH = ['日', '一', '二', '三', '四', '五', '六'];

function mapFitPackages(offers: TursoOfferResult[], pax: number): Map<string, FITPackage> {
  const packages = new Map<string, FITPackage>();
  for (const offer of offers) {
    if (!offer.departure_date || offer.price_per_person == null) continue;
    const current = packages.get(offer.departure_date);
    if (current && current.pricePerPerson <= offer.price_per_person) continue;
    packages.set(offer.departure_date, {
      file: offer.id,
      departDate: offer.departure_date,
      returnDate: offer.return_date || '',
      priceTotalTWD: offer.price_per_person * pax,
      pricePerPerson: offer.price_per_person,
      airline: offer.airline || 'Unknown',
      hotel: offer.hotel_name || offer.name || 'Unknown',
      baggageIncluded: true,
      pax,
    });
  }
  return packages;
}

function mapFlightOffers(offers: TursoOfferResult[]): Map<string, TursoOfferResult> {
  const flights = new Map<string, TursoOfferResult>();
  for (const offer of offers) {
    if (!offer.departure_date || offer.price_per_person == null) continue;
    const current = flights.get(offer.departure_date);
    if (!current || (current.price_per_person ?? Infinity) > offer.price_per_person) {
      flights.set(offer.departure_date, offer);
    }
  }
  return flights;
}

async function compareDates(options: CompareOptions): Promise<Result<DateComparison[]>> {
  // Load holiday calendar
  const calResult = await loadHolidayCalendar(options.market, parseInt(options.startDate.slice(0, 4), 10));
  if (!calResult.ok) return Result.err(calResult.error);
  const calendar = calResult.value;

  const commonFilters = {
    region: options.region,
    destination: options.destination,
    start: options.startDate,
    end: addDays(options.endDate, options.nights),
    limit: 500,
  };

  const packageOffers = await queryOffers({ ...commonFilters, type: 'package' });
  const flightOffers = await queryOffers({ ...commonFilters, type: 'flight' });
  if (packageOffers.length === 0 && flightOffers.length === 0) {
    return Result.err('Missing Turso offers for compare-dates. Import scrape output with npm run db:import:turso before running this command.');
  }

  const fitPackages = mapFitPackages(packageOffers, options.pax);
  const flightsByDate = mapFlightOffers(flightOffers);

  // Generate departure dates
  const departureDates: string[] = [];
  let current = options.startDate;
  while (current <= options.endDate) {
    departureDates.push(current);
    current = addDays(current, 1);
  }

  const comparisons: DateComparison[] = [];

  for (const departDate of departureDates) {
    const returnDate = addDays(departDate, options.nights);
    const dow = getDayOfWeek(departDate);
    const dowZh = DAY_NAMES_ZH[dow];

    // Calculate leave days
    const leaveResult = calculateLeaveDays(departDate, returnDate, calendar);
    let leaveDays = 0;
    let leaveDates: string[] = [];
    if (leaveResult.ok) {
      leaveDays = leaveResult.value.leaveDays;
      leaveDates = leaveResult.value.breakdown
        .filter(d => d.requiresLeave)
        .map(d => {
          const mm = d.date.slice(5, 7);
          const dd = d.date.slice(8, 10);
          return `${parseInt(mm)}/${parseInt(dd)}(${DAY_NAMES_ZH[d.dayOfWeek]})`;
        });
    }

    // FIT package
    const fit = fitPackages.get(departDate) || null;
    if (fit) {
      fit.returnDate = returnDate;
    }

    // Separate booking
    let separate: SeparateBooking | null = null;
    const outOffer = flightsByDate.get(departDate);
    const retOffer = flightsByDate.get(returnDate);

    if (outOffer || retOffer) {
      let outFlight: SeparateBooking['outboundFlight'] = null;
      let retFlight: SeparateBooking['returnFlight'] = null;

      if (outOffer && outOffer.price_per_person != null) {
        outFlight = {
          airline: outOffer.airline || outOffer.source_id,
          price: outOffer.price_per_person,
          priceTWD: outOffer.price_per_person * options.pax,
          baggage: false,
        };
      }

      if (retOffer && retOffer.price_per_person != null) {
        retFlight = {
          airline: retOffer.airline || retOffer.source_id,
          price: retOffer.price_per_person,
          priceTWD: retOffer.price_per_person * options.pax,
          baggage: false,
        };
      }

      const flightTotalTWD = (outFlight?.priceTWD || 0) + (retFlight?.priceTWD || 0);
      const hotelTotalTWD = options.hotelPerNight * options.nights;

      // Baggage: only charge for directions where baggage is NOT included
      let baggageDirs = 0;
      if (outFlight && !outFlight.baggage) baggageDirs++;
      if (retFlight && !retFlight.baggage) baggageDirs++;
      const baggageCostTWD = baggageDirs * options.baggageFeePerPersonPerDir * options.pax;

      const totalTWD = flightTotalTWD + hotelTotalTWD + baggageCostTWD;

      separate = {
        departDate,
        returnDate,
        outboundFlight: outFlight,
        returnFlight: retFlight,
        hotelTotalTWD,
        baggageCostTWD,
        totalTWD,
        pax: options.pax,
      };
    }

    // Compare
    let fitVsSeparate: number | null = null;
    if (fit && separate) {
      fitVsSeparate = fit.priceTotalTWD - separate.totalTWD;
    }

    comparisons.push({
      departDate,
      returnDate,
      dayOfWeek: dowZh,
      leaveDays,
      leaveDates,
      fit,
      separate,
      fitVsSeparate,
    });
  }

  return Result.ok(comparisons);
}

function formatComparisons(comparisons: DateComparison[], options: CompareOptions): string {
  const lines: string[] = [];

  lines.push(`## FIT vs 分開訂 全日期比較 (${options.nights}晚, ${options.pax}人)`);
  lines.push(`匯率: 1 USD ≈ ${EXCHANGE_RATES.USD_TWD} TWD | 飯店估: TWD ${options.hotelPerNight}/晚 | 行李: TWD ${options.baggageFeePerPersonPerDir}/人/方向`);
  lines.push('');

  // Main table
  lines.push('| 出發 | 回程 | 請假 | FIT(2人) | 分開訂(2人) | 差額 | 贏家 |');
  lines.push('|------|------|:----:|--------:|-----------:|-----:|------|');

  for (const c of comparisons) {
    const dep = `${c.departDate.slice(5)}(${c.dayOfWeek})`;
    const ret = c.returnDate.slice(5);
    const leave = `${c.leaveDays}天`;
    const fitPrice = c.fit ? c.fit.priceTotalTWD.toLocaleString() : '(無資料)';
    const sepPrice = c.separate ? `~${c.separate.totalTWD.toLocaleString()}` : '(無資料)';

    let diff = '-';
    let winner = '-';
    if (c.fitVsSeparate !== null) {
      if (c.fitVsSeparate > 0) {
        diff = `+${c.fitVsSeparate.toLocaleString()}`;
        winner = '分開訂';
      } else if (c.fitVsSeparate < 0) {
        diff = c.fitVsSeparate.toLocaleString();
        winner = 'FIT';
      } else {
        diff = '0';
        winner = '平手';
      }
    }

    lines.push(`| ${dep} | ${ret} | ${leave} | ${fitPrice} | ${sepPrice} | ${diff} | ${winner} |`);
  }

  // Detail breakdown for separate bookings
  lines.push('');
  lines.push('### 分開訂明細');
  lines.push('');
  lines.push('| 出發 | 去程 | TWD | 回程 | TWD | 飯店 | 行李 | 合計 |');
  lines.push('|------|------|----:|------|----:|-----:|-----:|-----:|');

  for (const c of comparisons) {
    if (!c.separate) {
      const dep = `${c.departDate.slice(5)}(${c.dayOfWeek})`;
      lines.push(`| ${dep} | - | - | - | - | - | - | (無資料) |`);
      continue;
    }
    const s = c.separate;
    const dep = `${c.departDate.slice(5)}(${c.dayOfWeek})`;
    const outAirline = s.outboundFlight ? s.outboundFlight.airline.slice(0, 12) : '-';
    const outTWD = s.outboundFlight ? s.outboundFlight.priceTWD.toLocaleString() : '-';
    const retAirline = s.returnFlight ? s.returnFlight.airline.slice(0, 12) : '-';
    const retTWD = s.returnFlight ? s.returnFlight.priceTWD.toLocaleString() : '-';
    const hotel = s.hotelTotalTWD.toLocaleString();
    const bag = s.baggageCostTWD > 0 ? s.baggageCostTWD.toLocaleString() : '0';
    const total = s.totalTWD.toLocaleString();

    lines.push(`| ${dep} | ${outAirline} | ${outTWD} | ${retAirline} | ${retTWD} | ${hotel} | ${bag} | ${total} |`);
  }

  // FIT detail
  lines.push('');
  lines.push('### FIT 明細');
  lines.push('');
  lines.push('| 出發 | 價格(2人) | 每人 | 航空 | 飯店 | 行李 |');
  lines.push('|------|--------:|-----:|------|------|------|');

  for (const c of comparisons) {
    const dep = `${c.departDate.slice(5)}(${c.dayOfWeek})`;
    if (!c.fit) {
      lines.push(`| ${dep} | (無資料) | - | - | - | - |`);
      continue;
    }
    const f = c.fit;
    lines.push(`| ${dep} | ${f.priceTotalTWD.toLocaleString()} | ${f.pricePerPerson.toLocaleString()} | ${f.airline} | ${f.hotel} | 含20kg |`);
  }

  // Leave day breakdown
  lines.push('');
  lines.push('### 請假明細');
  lines.push('');
  lines.push('| 出發 | 請假天數 | 請假日期 |');
  lines.push('|------|:-------:|---------|');

  for (const c of comparisons) {
    const dep = `${c.departDate.slice(5)}(${c.dayOfWeek})`;
    lines.push(`| ${dep} | ${c.leaveDays}天 | ${c.leaveDates.join(', ') || '不用請假'} |`);
  }

  // Recommendation
  const withBothData = comparisons.filter(c => c.fit && c.separate);
  if (withBothData.length > 0) {
    const cheapestFIT = [...comparisons].filter(c => c.fit).sort((a, b) => a.fit!.priceTotalTWD - b.fit!.priceTotalTWD)[0];
    const cheapestSep = [...comparisons].filter(c => c.separate).sort((a, b) => a.separate!.totalTWD - b.separate!.totalTWD)[0];
    const leastLeave = [...comparisons].sort((a, b) => a.leaveDays - b.leaveDays)[0];

    lines.push('');
    lines.push('### 建議');
    if (cheapestFIT) lines.push(`- **最便宜FIT**: ${cheapestFIT.departDate.slice(5)}(${cheapestFIT.dayOfWeek}) TWD ${cheapestFIT.fit!.priceTotalTWD.toLocaleString()}`);
    if (cheapestSep) lines.push(`- **最便宜分開訂**: ${cheapestSep.departDate.slice(5)}(${cheapestSep.dayOfWeek}) ~TWD ${cheapestSep.separate!.totalTWD.toLocaleString()}`);
    lines.push(`- **最少請假**: ${leastLeave.departDate.slice(5)}(${leastLeave.dayOfWeek}) ${leastLeave.leaveDays}天`);
  }

  return lines.join('\n');
}

// CLI
function parseArgs(args: string[]): CompareOptions & { help: boolean } {
  const result = {
    startDate: '',
    endDate: '',
    nights: 4,
    hotelPerNight: 3200,
    market: 'taiwan',
    region: undefined as string | undefined,
    destination: undefined as string | undefined,
    pax: DEFAULTS.pax as number,
    baggageFeePerPersonPerDir: DEFAULT_LCC_BAGGAGE_FEE,
    help: false,
  };

  for (let i = 0; i < args.length; i++) {
    switch (args[i]) {
      case '--start': case '-s': result.startDate = args[++i]; break;
      case '--end': case '-e': result.endDate = args[++i]; break;
      case '--nights': case '-n': result.nights = parseInt(args[++i], 10); break;
      case '--hotel-per-night': result.hotelPerNight = parseInt(args[++i], 10); break;
      case '--market': case '-m': result.market = args[++i]; break;
      case '--region': result.region = args[++i]; break;
      case '--destination': result.destination = args[++i]; break;
      case '--pax': result.pax = parseInt(args[++i], 10); break;
      case '--baggage-fee': result.baggageFeePerPersonPerDir = parseInt(args[++i], 10); break;
      case '--help': case '-h': result.help = true; break;
    }
  }

  return result;
}

async function main(): Promise<void> {
  const args = parseArgs(process.argv.slice(2));

  if (args.help || !args.startDate || !args.endDate) {
    console.log(`
Compare Dates CLI - FIT vs Separate Booking Comparison

Usage:
  npx ts-node src/cli/compare-dates.ts --start 2026-02-24 --end 2026-02-28 --nights 4

Options:
  --start, -s          First departure date (YYYY-MM-DD) [required]
  --end, -e            Last departure date (YYYY-MM-DD) [required]
  --nights, -n         Number of hotel nights (default: 4)
  --hotel-per-night    Hotel cost TWD/night/room (default: 3200)
  --market, -m         Holiday country/market in Turso (default: taiwan)
  --region             Offer region filter (optional)
  --destination        Offer destination filter (optional)
  --pax                Number of passengers (default: 2)
  --baggage-fee        LCC baggage fee TWD/person/direction (default: 1750)
  --help, -h           Show this help

Data sources:
  Turso offers table (populate with npm run db:import:turso)
`);
    process.exit(args.help ? 0 : 1);
  }

  const result = await compareDates(args);
  if (!result.ok) {
    console.error(`Error: ${result.error}`);
    process.exit(1);
  }

  console.log(formatComparisons(result.value, args));
}

main().catch((err) => {
  console.error(err instanceof Error ? err.message : String(err));
  process.exit(1);
});

export { compareDates, CompareOptions, DateComparison, FITPackage, SeparateBooking };
