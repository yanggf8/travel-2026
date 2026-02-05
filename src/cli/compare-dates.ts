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

import * as fs from 'fs';
import * as path from 'path';
import { normalizeFlightData, scanFlightFiles, FlightSearchResult } from '../utils/flight-normalizer';
import { loadHolidayCalendar, calculateLeaveDays, LeaveDayResult, HolidayCalendar } from '../utils/leave-calculator';
import { EXCHANGE_RATES, convertToTWD, DEFAULTS, DEFAULT_LCC_BAGGAGE_FEE } from '../config/constants';
import { Result } from '../types';

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
  calendarPath: string;
  dataDir: string;
  pax: number;
  baggageFeePerPersonPerDir: number;
}

const DAY_NAMES_ZH = ['日', '一', '二', '三', '四', '五', '六'];

function addDays(dateStr: string, days: number): string {
  const [y, m, d] = dateStr.split('-').map(Number);
  const date = new Date(y, m - 1, d + days);
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
}

function getDayOfWeek(dateStr: string): number {
  const [y, m, d] = dateStr.split('-').map(Number);
  return new Date(y, m - 1, d).getDay();
}

function parseLionTravelFIT(filePath: string): FITPackage | null {
  try {
    const content = fs.readFileSync(filePath, 'utf-8');
    const data = JSON.parse(content);
    const rawText: string = data.raw_text || '';

    // Extract price - look for patterns like "40,740" or "每人$20,370"
    let priceTotalTWD = 0;
    let pricePerPerson = 0;

    // Try to find "每人費用" or per-person price
    const perPersonMatch = rawText.match(/每人費用[^\d]*?([\d,]+)/);
    if (perPersonMatch) {
      pricePerPerson = parseInt(perPersonMatch[1].replace(/,/g, ''), 10);
      priceTotalTWD = pricePerPerson * 2;
    }

    // Try "售價" or total price patterns
    if (!priceTotalTWD) {
      const priceMatch = rawText.match(/售價[^\d]*?([\d,]+)/);
      if (priceMatch) {
        pricePerPerson = parseInt(priceMatch[1].replace(/,/g, ''), 10);
        priceTotalTWD = pricePerPerson * 2;
      }
    }

    // Try "TWD XX,XXX" patterns - find per-person and total
    if (!priceTotalTWD) {
      const twdPrices = [...rawText.matchAll(/TWD\s*([\d,]+)/g)]
        .map(m => parseInt(m[1].replace(/,/g, ''), 10))
        .filter(p => p >= 10000 && p <= 200000);
      if (twdPrices.length >= 2) {
        // Smallest is per-person, next is total (2 pax)
        twdPrices.sort((a, b) => a - b);
        pricePerPerson = twdPrices[0];
        priceTotalTWD = twdPrices[1];
        // Sanity check: total should be ~2x per-person
        if (Math.abs(priceTotalTWD - pricePerPerson * 2) > pricePerPerson * 0.1) {
          // If not, assume first is per-person
          priceTotalTWD = pricePerPerson * 2;
        }
      } else if (twdPrices.length === 1) {
        pricePerPerson = twdPrices[0];
        priceTotalTWD = pricePerPerson * 2;
      }
    }

    // Try extracted price
    if (!priceTotalTWD && data.extracted?.price) {
      const ep = data.extracted.price;
      if (ep.per_person) {
        pricePerPerson = ep.per_person;
        priceTotalTWD = pricePerPerson * 2;
      } else if (ep.total) {
        priceTotalTWD = ep.total;
        pricePerPerson = Math.round(priceTotalTWD / 2);
      }
    }

    // Extract airline - specific names first, generic patterns last
    let airline = 'Unknown';
    const airlinePatterns = [
      /(泰國獅子航空|長榮航空|中華航空|國泰航空|台灣虎航|星宇航空|樂桃航空)/,
      /(Thai Lion Air|EVA Air|China Airlines|Cathay Pacific|Peach Aviation|Tigerair Taiwan|STARLUX|Scoot)/i,
      /(Thai Lion|EVA|Cathay|Peach|Tigerair|STARLUX|Scoot)/i,
    ];
    for (const pat of airlinePatterns) {
      const m = rawText.match(pat);
      if (m) { airline = m[1].trim(); break; }
    }

    // Fallback: check extracted flight
    if (airline === 'Unknown' && data.extracted?.flight?.airline) {
      airline = data.extracted.flight.airline;
    }

    // Extract hotel
    let hotel = 'Unknown';
    const hotelPatterns = [
      /住宿[：:]\s*(.+)/,
      /飯店[：:]\s*(.+)/,
      /(Just Sleep|Hankyu Respire|TAVINOS|Dormy Inn|Toyoko Inn|APA Hotel)/i,
      /(捷絲旅|阪急レスパイア|阪急RESPIRE)/,
    ];
    for (const pat of hotelPatterns) {
      const m = rawText.match(pat);
      if (m) { hotel = m[1].trim(); break; }
    }
    if (hotel === 'Unknown' && data.extracted?.hotel?.name) {
      hotel = data.extracted.hotel.name;
    }

    // Extract dates from URL or content
    const url: string = data.url || '';
    let departDate = '';
    let returnDate = '';

    // Try to detect from filename
    const fileBase = path.basename(filePath);
    const datePatterns = fileBase.match(/feb(\d{2})/i);
    if (datePatterns) {
      const day = parseInt(datePatterns[1], 10);
      departDate = `2026-02-${String(day).padStart(2, '0')}`;
    }

    if (!priceTotalTWD) return null;

    return {
      file: filePath,
      departDate,
      returnDate: returnDate || (departDate ? addDays(departDate, 4) : ''),
      priceTotalTWD,
      pricePerPerson,
      airline,
      hotel,
      baggageIncluded: true, // FIT always includes baggage
      pax: 2,
    };
  } catch {
    return null;
  }
}

function scanFITPackages(dataDir: string): Map<string, FITPackage> {
  const packages = new Map<string, FITPackage>();
  const dir = path.isAbsolute(dataDir) ? dataDir : path.resolve(process.cwd(), dataDir);

  const files = fs.readdirSync(dir).filter(f =>
    f.startsWith('liontravel-') && f.includes('fresh') && f.endsWith('.json')
  );

  for (const file of files) {
    const filePath = path.join(dir, file);
    const pkg = parseLionTravelFIT(filePath);
    if (pkg && pkg.departDate) {
      packages.set(pkg.departDate, pkg);
    }
  }

  return packages;
}

function compareDates(options: CompareOptions): Result<DateComparison[]> {
  // Load holiday calendar
  const calResult = loadHolidayCalendar(options.calendarPath);
  if (!calResult.ok) return Result.err(calResult.error);
  const calendar = calResult.value;

  // Scan flight files
  const { outbound, return_ } = scanFlightFiles(options.dataDir);

  // Scan FIT packages
  const fitPackages = scanFITPackages(options.dataDir);

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
    const outFile = outbound.get(departDate);
    const retFile = return_.get(returnDate);

    if (outFile || retFile) {
      let outFlight: SeparateBooking['outboundFlight'] = null;
      let retFlight: SeparateBooking['returnFlight'] = null;

      if (outFile) {
        const outResult = normalizeFlightData(outFile);
        if (outResult.ok && outResult.value.cheapestAny) {
          const f = outResult.value.cheapestAny;
          outFlight = {
            airline: f.airline,
            price: f.priceTotal,
            priceTWD: f.priceTotalTWD,
            baggage: f.baggageIncluded,
          };
        }
      }

      if (retFile) {
        const retResult = normalizeFlightData(retFile);
        if (retResult.ok && retResult.value.cheapestAny) {
          const f = retResult.value.cheapestAny;
          retFlight = {
            airline: f.airline,
            price: f.priceTotal,
            priceTWD: f.priceTotalTWD,
            baggage: f.baggageIncluded,
          };
        }
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
    calendarPath: 'data/holidays/taiwan-2026.json',
    dataDir: 'data',
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
      case '--calendar': case '-c': result.calendarPath = args[++i]; break;
      case '--data-dir': result.dataDir = args[++i]; break;
      case '--pax': result.pax = parseInt(args[++i], 10); break;
      case '--baggage-fee': result.baggageFeePerPersonPerDir = parseInt(args[++i], 10); break;
      case '--help': case '-h': result.help = true; break;
    }
  }

  return result;
}

function main(): void {
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
  --calendar, -c       Holiday calendar path (default: data/holidays/taiwan-2026.json)
  --data-dir           Directory with scraped data (default: data)
  --pax                Number of passengers (default: 2)
  --baggage-fee        LCC baggage fee TWD/person/direction (default: 1750)
  --help, -h           Show this help

Data sources:
  FIT packages:    data/liontravel-feb*-fresh.json
  Outbound flights: data/trip-feb*-out.json, data/trip-flights-feb*-out*.json
  Return flights:   data/trip-feb*-return.json, data/trip-flights-mar*-return*.json, data/trip-mar*-return.json
`);
    process.exit(args.help ? 0 : 1);
  }

  const result = compareDates(args);
  if (!result.ok) {
    console.error(`Error: ${result.error}`);
    process.exit(1);
  }

  console.log(formatComparisons(result.value, args));
}

main();

export { compareDates, CompareOptions, DateComparison, FITPackage, SeparateBooking };
