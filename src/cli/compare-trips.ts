#!/usr/bin/env npx ts-node
/**
 * Trip Comparison CLI
 *
 * Compare multiple trip options with normalized costs, leave days, and recommendations.
 *
 * Usage:
 *   npx ts-node src/cli/compare-trips.ts --input data/trip-comparison.json
 *   npx ts-node src/cli/compare-trips.ts --trips '...' --calendar data/holidays/taiwan-2026.json
 */

import * as fs from 'fs';
import * as path from 'path';
import {
  loadHolidayCalendar,
  calculateLeaveDays,
  HolidayCalendar,
  LeaveDayResult,
} from '../utils/leave-calculator';
import { Result } from '../types';

// Types
interface FlightInfo {
  airline: string;
  flightNumber?: string;
  departure: string;
  arrival: string;
  isLCC: boolean;
}

interface HotelInfo {
  name: string;
  pricePerNight: number;
  nights: number;
}

interface TripOption {
  id: string;
  type: 'package' | 'separate';
  startDate: string;
  endDate: string;
  pax: number;

  // For packages
  packagePrice?: number;

  // For separate bookings
  outboundFlight?: FlightInfo;
  returnFlight?: FlightInfo;
  flightPriceTotal?: number;
  hotel?: HotelInfo;
  hotelPriceTotal?: number;

  // Adjustments
  baggageFee?: number; // Per person, for LCC
  currency: string;
  notes?: string;
}

interface ComparisonInput {
  trips: TripOption[];
  calendarPath: string;
  defaultBaggageFee?: number; // Default LCC baggage fee per person
}

interface TripAnalysis {
  id: string;
  type: 'package' | 'separate';
  startDate: string;
  endDate: string;
  totalDays: number;
  leaveDays: number;
  pax: number;

  // Cost breakdown
  basePrice: number;
  baggageFee: number;
  totalPrice: number;
  pricePerPerson: number;
  pricePerLeaveDay: number;

  // Details
  flightInfo?: string;
  hotelInfo?: string;
  notes?: string;

  // Leave breakdown
  leaveBreakdown: LeaveDayResult;
}

interface ComparisonResult {
  analyses: TripAnalysis[];
  recommendation: {
    bestValue: string;
    leastLeave: string;
    cheapest: string;
  };
  summary: string;
}

const DEFAULT_BAGGAGE_FEE = 1750; // TWD per person for 20kg checked bag

function analyzeTripOption(
  option: TripOption,
  calendar: HolidayCalendar,
  defaultBaggageFee: number
): Result<TripAnalysis> {
  // Calculate leave days
  const leaveResult = calculateLeaveDays(option.startDate, option.endDate, calendar);
  if (!leaveResult.ok) {
    return Result.err(`Failed to calculate leave for ${option.id}: ${leaveResult.error}`);
  }

  const leave = leaveResult.value;

  // Calculate costs
  let basePrice: number;
  let baggageFee = 0;
  let flightInfo: string | undefined;
  let hotelInfo: string | undefined;

  if (option.type === 'package') {
    basePrice = option.packagePrice || 0;
  } else {
    // Separate booking
    const flightTotal = option.flightPriceTotal || 0;
    const hotelTotal = option.hotelPriceTotal || 0;
    basePrice = flightTotal + hotelTotal;

    // Check for LCC baggage fees
    const isOutboundLCC = option.outboundFlight?.isLCC ?? false;
    const isReturnLCC = option.returnFlight?.isLCC ?? false;
    if (isOutboundLCC || isReturnLCC) {
      const feePerPerson = option.baggageFee ?? defaultBaggageFee;
      // Both directions need baggage
      const directions = (isOutboundLCC ? 1 : 0) + (isReturnLCC ? 1 : 0);
      baggageFee = feePerPerson * directions * option.pax;
    }

    // Flight info string
    if (option.outboundFlight && option.returnFlight) {
      flightInfo = `${option.outboundFlight.airline} → ${option.returnFlight.airline}`;
    }

    // Hotel info string
    if (option.hotel) {
      hotelInfo = `${option.hotel.name} (${option.hotel.nights}晚)`;
    }
  }

  const totalPrice = basePrice + baggageFee;
  const pricePerPerson = totalPrice / option.pax;
  const pricePerLeaveDay = leave.leaveDays > 0 ? totalPrice / leave.leaveDays : totalPrice;

  return Result.ok({
    id: option.id,
    type: option.type,
    startDate: option.startDate,
    endDate: option.endDate,
    totalDays: leave.totalDays,
    leaveDays: leave.leaveDays,
    pax: option.pax,
    basePrice,
    baggageFee,
    totalPrice,
    pricePerPerson,
    pricePerLeaveDay,
    flightInfo,
    hotelInfo,
    notes: option.notes,
    leaveBreakdown: leave,
  });
}

function compareTrips(input: ComparisonInput): Result<ComparisonResult> {
  // Load calendar
  const calendarResult = loadHolidayCalendar(input.calendarPath);
  if (!calendarResult.ok) {
    return Result.err(calendarResult.error);
  }

  const calendar = calendarResult.value;
  const defaultBaggageFee = input.defaultBaggageFee ?? DEFAULT_BAGGAGE_FEE;
  const analyses: TripAnalysis[] = [];

  // Analyze each trip
  for (const trip of input.trips) {
    const analysis = analyzeTripOption(trip, calendar, defaultBaggageFee);
    if (!analysis.ok) {
      return Result.err(analysis.error);
    }
    analyses.push(analysis.value);
  }

  // Find recommendations
  const byValue = [...analyses].sort((a, b) => a.pricePerLeaveDay - b.pricePerLeaveDay);
  const byLeave = [...analyses].sort((a, b) => a.leaveDays - b.leaveDays);
  const byPrice = [...analyses].sort((a, b) => a.totalPrice - b.totalPrice);

  const recommendation = {
    bestValue: byValue[0].id,
    leastLeave: byLeave[0].id,
    cheapest: byPrice[0].id,
  };

  // Generate summary
  const summaryLines: string[] = [];
  summaryLines.push('## Trip Comparison Summary\n');

  // Table
  summaryLines.push('| Option | Type | Dates | Leave | Total | $/Leave |');
  summaryLines.push('|--------|------|-------|:-----:|------:|--------:|');

  for (const a of analyses) {
    const dates = `${a.startDate.slice(5)} → ${a.endDate.slice(5)}`;
    const typeLabel = a.type === 'package' ? '套餐' : '分開訂';
    const totalStr = `${a.totalPrice.toLocaleString()}`;
    const perLeaveStr = `${Math.round(a.pricePerLeaveDay).toLocaleString()}`;
    const leaveStr = `${a.leaveDays}天`;
    summaryLines.push(`| ${a.id} | ${typeLabel} | ${dates} | ${leaveStr} | ${totalStr} | ${perLeaveStr} |`);
  }

  summaryLines.push('');
  summaryLines.push('### Recommendations');
  summaryLines.push(`- **Best Value ($/Leave)**: ${recommendation.bestValue}`);
  summaryLines.push(`- **Least Leave Days**: ${recommendation.leastLeave}`);
  summaryLines.push(`- **Cheapest Total**: ${recommendation.cheapest}`);

  return Result.ok({
    analyses,
    recommendation,
    summary: summaryLines.join('\n'),
  });
}

function formatDetailedAnalysis(analysis: TripAnalysis): string {
  const lines: string[] = [];

  lines.push(`### ${analysis.id}`);
  lines.push(`- **Type**: ${analysis.type === 'package' ? '套餐' : '分開訂'}`);
  lines.push(`- **Dates**: ${analysis.startDate} → ${analysis.endDate} (${analysis.totalDays} days)`);
  lines.push(`- **Leave Days**: ${analysis.leaveDays}`);

  if (analysis.flightInfo) {
    lines.push(`- **Flight**: ${analysis.flightInfo}`);
  }
  if (analysis.hotelInfo) {
    lines.push(`- **Hotel**: ${analysis.hotelInfo}`);
  }

  lines.push(`- **Base Price**: ${analysis.basePrice.toLocaleString()}`);
  if (analysis.baggageFee > 0) {
    lines.push(`- **Baggage Fee**: +${analysis.baggageFee.toLocaleString()} (LCC)`);
  }
  lines.push(`- **Total**: ${analysis.totalPrice.toLocaleString()}`);
  lines.push(`- **Per Person**: ${analysis.pricePerPerson.toLocaleString()}`);
  lines.push(`- **Per Leave Day**: ${Math.round(analysis.pricePerLeaveDay).toLocaleString()}`);

  if (analysis.notes) {
    lines.push(`- **Notes**: ${analysis.notes}`);
  }

  return lines.join('\n');
}

// CLI
function printUsage(): void {
  console.log(`
Trip Comparison CLI

Usage:
  npx ts-node src/cli/compare-trips.ts --input <file.json>
  npx ts-node src/cli/compare-trips.ts --trips '<json>' --calendar <calendar.json>

Options:
  --input, -i       Input JSON file with trips and calendar path
  --trips, -t       Inline JSON array of trip options
  --calendar, -c    Path to holiday calendar (default: data/holidays/taiwan-2026.json)
  --detailed, -d    Show detailed breakdown for each trip
  --help, -h        Show this help

Input JSON format:
{
  "trips": [
    {
      "id": "Feb24-Package",
      "type": "package",
      "startDate": "2026-02-24",
      "endDate": "2026-02-28",
      "pax": 2,
      "packagePrice": 40740,
      "currency": "TWD"
    },
    {
      "id": "Feb24-Separate",
      "type": "separate",
      "startDate": "2026-02-24",
      "endDate": "2026-02-28",
      "pax": 2,
      "outboundFlight": { "airline": "AirAsia", "isLCC": true },
      "returnFlight": { "airline": "Peach", "isLCC": true },
      "flightPriceTotal": 26000,
      "hotel": { "name": "Onyado Nono", "pricePerNight": 3042, "nights": 4 },
      "hotelPriceTotal": 12168,
      "currency": "TWD"
    }
  ],
  "calendarPath": "data/holidays/taiwan-2026.json"
}
`);
}

function parseArgs(args: string[]): {
  input?: string;
  trips?: string;
  calendar: string;
  detailed: boolean;
  help: boolean;
} {
  const result = {
    input: undefined as string | undefined,
    trips: undefined as string | undefined,
    calendar: 'data/holidays/taiwan-2026.json',
    detailed: false,
    help: false,
  };

  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    switch (arg) {
      case '--input':
      case '-i':
        result.input = args[++i];
        break;
      case '--trips':
      case '-t':
        result.trips = args[++i];
        break;
      case '--calendar':
      case '-c':
        result.calendar = args[++i];
        break;
      case '--detailed':
      case '-d':
        result.detailed = true;
        break;
      case '--help':
      case '-h':
        result.help = true;
        break;
    }
  }

  return result;
}

async function main(): Promise<void> {
  const args = parseArgs(process.argv.slice(2));

  if (args.help) {
    printUsage();
    process.exit(0);
  }

  let input: ComparisonInput;

  if (args.input) {
    // Load from file
    try {
      const content = fs.readFileSync(args.input, 'utf-8');
      input = JSON.parse(content);
    } catch (e) {
      console.error(`Failed to load input file: ${e instanceof Error ? e.message : String(e)}`);
      process.exit(1);
    }
  } else if (args.trips) {
    // Parse inline JSON
    try {
      const trips = JSON.parse(args.trips);
      input = {
        trips,
        calendarPath: args.calendar,
      };
    } catch (e) {
      console.error(`Failed to parse trips JSON: ${e instanceof Error ? e.message : String(e)}`);
      process.exit(1);
    }
  } else {
    printUsage();
    process.exit(1);
  }

  const result = compareTrips(input);

  if (!result.ok) {
    console.error(`Comparison failed: ${result.error}`);
    process.exit(1);
  }

  console.log(result.value.summary);

  if (args.detailed) {
    console.log('\n## Detailed Breakdown\n');
    for (const analysis of result.value.analyses) {
      console.log(formatDetailedAnalysis(analysis));
      console.log('');
    }
  }
}

main().catch(console.error);

export { compareTrips, TripOption, ComparisonInput, ComparisonResult, TripAnalysis };
