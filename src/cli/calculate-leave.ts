#!/usr/bin/env npx ts-node
/**
 * Leave Calculator CLI
 *
 * Calculate leave days needed for trip planning around Taiwan holidays.
 *
 * Usage:
 *   npx ts-node src/cli/calculate-leave.ts --start YYYY-MM-DD --end YYYY-MM-DD [--market tw|jp]
 *   npx ts-node src/cli/calculate-leave.ts --compare "YYYY-MM-DD,YYYY-MM-DD,..." --duration N [--market tw|jp]
 *
 * Examples:
 *   npx ts-node src/cli/calculate-leave.ts --start 2026-02-24 --end 2026-02-28
 *   npx ts-node src/cli/calculate-leave.ts --compare "2026-02-24,2026-02-25,2026-02-26,2026-02-27" --duration 5
 */

import {
  calculateLeave,
  getHolidaysInRange,
  type LeaveResult,
} from '../utils/holiday-calculator';
import { addDays } from '../utils/date-utils';

// Helper: Format leave plan for display
function formatLeavePlan(plan: LeaveResult): string {
  return `請假天數: ${plan.leaveDaysNeeded}天
總天數: ${plan.totalDays}天
週末: ${plan.weekendDays}天
假日: ${plan.holidayDays}天`;
}

// Helper: Compare departure dates
function compareDepartureDates(dates: string[], duration: number, market: string) {
  const results = dates.map(date => {
    const endDate = addDays(date, duration - 1);
    const plan = calculateLeave({ startDate: date, endDate, market });
    const holidays = getHolidaysInRange(date, endDate, market);
    return {
      date,
      returnDate: endDate,
      leaveDays: plan.leaveDaysNeeded,
      holidays: holidays.map(h => h.name),
    };
  });
  return results.sort((a, b) => a.leaveDays - b.leaveDays);
}

const HELP = `
Leave Calculator CLI - Calculate leave days for trip planning

Usage:
  npx ts-node src/cli/calculate-leave.ts <options>

Options:
  --start <YYYY-MM-DD>    Trip start date (required for single calculation)
  --end <YYYY-MM-DD>      Trip end date (required for single calculation)
  --market <tw|jp>        Holiday market (default: tw)
  --compare <dates>       Comma-separated departure dates to compare
  --duration <N>          Trip duration in days (required with --compare)
  --holidays <range>      Show holidays in date range (e.g., "2026-02-01,2026-03-31")
  --json                  Output as JSON
  --help                  Show this help message

Examples:
  # Calculate leave for specific dates
  npx ts-node src/cli/calculate-leave.ts --start 2026-02-24 --end 2026-02-28

  # Compare multiple departure dates
  npx ts-node src/cli/calculate-leave.ts --compare "2026-02-24,2026-02-25,2026-02-26,2026-02-27" --duration 5

  # List holidays in February-March 2026
  npx ts-node src/cli/calculate-leave.ts --holidays "2026-02-01,2026-03-31"
`;

interface CliArgs {
  start?: string;
  end?: string;
  market: 'tw' | 'jp';
  compare?: string[];
  duration?: number;
  holidays?: { start: string; end: string };
  json: boolean;
  help: boolean;
}

function parseArgs(args: string[]): CliArgs {
  const result: CliArgs = {
    market: 'tw',
    json: false,
    help: false,
  };

  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    switch (arg) {
      case '--start':
        result.start = args[++i];
        break;
      case '--end':
        result.end = args[++i];
        break;
      case '--market':
        result.market = args[++i] as 'tw' | 'jp';
        break;
      case '--compare':
        result.compare = args[++i].split(',').map(d => d.trim());
        break;
      case '--duration':
        result.duration = parseInt(args[++i], 10);
        break;
      case '--holidays': {
        const [start, end] = args[++i].split(',').map(d => d.trim());
        result.holidays = { start, end };
        break;
      }
      case '--json':
        result.json = true;
        break;
      case '--help':
      case '-h':
        result.help = true;
        break;
    }
  }

  return result;
}

function validateDate(dateStr: string | undefined, name: string): string {
  if (!dateStr) {
    throw new Error(`Missing required argument: ${name}`);
  }
  if (!/^\d{4}-\d{2}-\d{2}$/.test(dateStr)) {
    throw new Error(`Invalid date format for ${name}: ${dateStr} (expected YYYY-MM-DD)`);
  }
  return dateStr;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));

  if (args.help || process.argv.length <= 2) {
    console.log(HELP);
    process.exit(0);
  }

  // Mode 1: Show holidays in range
  if (args.holidays) {
    const holidays = getHolidaysInRange(args.holidays.start, args.holidays.end, args.market);

    if (args.json) {
      console.log(JSON.stringify(holidays, null, 2));
    } else {
      console.log(`\n${args.market.toUpperCase()} 假日 (${args.holidays.start} ~ ${args.holidays.end}):\n`);
      if (holidays.length === 0) {
        console.log('  (無國定假日)');
      } else {
        for (const h of holidays) {
          console.log(`  ${h.date}: ${h.name} (${h.name_en})`);
        }
      }
      console.log();
    }
    return;
  }

  // Mode 2: Compare multiple departure dates
  if (args.compare) {
    if (!args.duration) {
      throw new Error('--duration is required when using --compare');
    }

    const results = compareDepartureDates(args.compare, args.duration, args.market);

    if (args.json) {
      console.log(JSON.stringify(results, null, 2));
    } else {
      console.log(`\n比較出發日 (${args.duration}天行程):\n`);
      console.log('| 出發日 | 回程日 | 請假天數 | 涵蓋假日 |');
      console.log('|--------|--------|---------|---------|');
      for (const r of results) {
        const holidayStr = r.holidays.length > 0 ? r.holidays.join(', ') : '-';
        const marker = r === results[0] ? ' 🏆' : '';
        console.log(`| ${r.date} | ${r.returnDate} | ${r.leaveDays}天${marker} | ${holidayStr} |`);
      }
      console.log(`\n🏆 = 需請假天數最少\n`);
    }
    return;
  }

  // Mode 3: Single date range calculation
  const start = validateDate(args.start, '--start');
  const end = validateDate(args.end, '--end');

  const plan = calculateLeave({ startDate: start, endDate: end, market: args.market });

  if (args.json) {
    console.log(JSON.stringify(plan, null, 2));
  } else {
    console.log('\n' + formatLeavePlan(plan) + '\n');
  }
}

main().catch(err => {
  console.error('Error:', err.message);
  process.exit(1);
});
