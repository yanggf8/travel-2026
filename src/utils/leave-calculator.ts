/**
 * Leave day calculator for trip planning.
 *
 * Calculates the number of leave days required for a trip,
 * accounting for weekends and public holidays.
 */

import * as fs from 'fs';
import * as path from 'path';
import { Result } from '../types';

// Types
export interface HolidayEntry {
  name: string;
  name_en: string;
  type: 'national' | 'substitute';
}

export interface MakeupWorkday {
  name: string;
  name_en: string;
  for_holiday: string;
}

export interface HolidayCalendar {
  country: string;
  year: number;
  description: string;
  holidays: Record<string, HolidayEntry>;
  makeup_workdays: Record<string, MakeupWorkday>;
}

export interface DayDetail {
  date: string;
  dayOfWeek: number; // 0=Sun, 1=Mon, ..., 6=Sat
  dayName: string;
  isWeekend: boolean;
  isHoliday: boolean;
  isMakeupWorkday: boolean;
  holidayName?: string;
  requiresLeave: boolean;
}

export interface LeaveDayResult {
  startDate: string;
  endDate: string;
  totalDays: number;
  leaveDays: number;
  weekendDays: number;
  holidayDays: number;
  breakdown: DayDetail[];
}

const DAY_NAMES = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
const DAY_NAMES_ZH = ['日', '一', '二', '三', '四', '五', '六'];

/**
 * Load holiday calendar from JSON file.
 */
export function loadHolidayCalendar(filePath: string): Result<HolidayCalendar> {
  try {
    const absolutePath = path.isAbsolute(filePath)
      ? filePath
      : path.resolve(process.cwd(), filePath);
    const content = fs.readFileSync(absolutePath, 'utf-8');
    return Result.ok(JSON.parse(content) as HolidayCalendar);
  } catch (e) {
    return Result.err(`Failed to load holiday calendar: ${e instanceof Error ? e.message : String(e)}`);
  }
}

/**
 * Get holiday calendar for a specific year.
 * Looks in data/holidays/{country}-{year}.json
 */
export function getHolidayCalendarForYear(
  country: string,
  year: number,
  dataDir: string = 'data/holidays'
): Result<HolidayCalendar> {
  const filePath = path.join(dataDir, `${country}-${year}.json`);
  return loadHolidayCalendar(filePath);
}

/**
 * Parse date string (YYYY-MM-DD) to Date object.
 */
function parseDate(dateStr: string): Date {
  const [year, month, day] = dateStr.split('-').map(Number);
  return new Date(year, month - 1, day);
}

/**
 * Format Date to MM-DD string for calendar lookup.
 */
function formatMonthDay(date: Date): string {
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  return `${month}-${day}`;
}

/**
 * Format Date to YYYY-MM-DD string.
 */
function formatDate(date: Date): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
}

/**
 * Check if a date is a weekend (Saturday or Sunday).
 */
function isWeekend(date: Date): boolean {
  const day = date.getDay();
  return day === 0 || day === 6;
}

/**
 * Calculate leave days required for a trip.
 *
 * @param startDate - Trip start date (YYYY-MM-DD)
 * @param endDate - Trip end date (YYYY-MM-DD)
 * @param calendar - Holiday calendar for the year
 * @returns LeaveDayResult with breakdown
 */
export function calculateLeaveDays(
  startDate: string,
  endDate: string,
  calendar: HolidayCalendar
): Result<LeaveDayResult> {
  const start = parseDate(startDate);
  const end = parseDate(endDate);

  if (start > end) {
    return Result.err('Start date must be before or equal to end date');
  }

  const breakdown: DayDetail[] = [];
  let leaveDays = 0;
  let weekendDays = 0;
  let holidayDays = 0;

  const current = new Date(start);
  while (current <= end) {
    const dateStr = formatDate(current);
    const monthDay = formatMonthDay(current);
    const dayOfWeek = current.getDay();
    const weekend = isWeekend(current);
    const holiday = calendar.holidays[monthDay];
    const makeupWorkday = calendar.makeup_workdays[monthDay];

    const isHoliday = !!holiday;
    const isMakeupWorkday = !!makeupWorkday;

    // Determine if leave is required:
    // - Weekends: no leave (unless makeup workday)
    // - Holidays: no leave
    // - Makeup workdays: requires leave (even if weekend)
    // - Regular weekdays: requires leave
    let requiresLeave: boolean;
    if (isMakeupWorkday) {
      // Makeup workday always requires leave
      requiresLeave = true;
    } else if (isHoliday || weekend) {
      // Holiday or weekend doesn't require leave
      requiresLeave = false;
    } else {
      // Regular weekday requires leave
      requiresLeave = true;
    }

    if (requiresLeave) {
      leaveDays++;
    }
    if (weekend && !isMakeupWorkday) {
      weekendDays++;
    }
    if (isHoliday) {
      holidayDays++;
    }

    breakdown.push({
      date: dateStr,
      dayOfWeek,
      dayName: DAY_NAMES[dayOfWeek],
      isWeekend: weekend,
      isHoliday,
      isMakeupWorkday,
      holidayName: holiday?.name,
      requiresLeave,
    });

    current.setDate(current.getDate() + 1);
  }

  return Result.ok({
    startDate,
    endDate,
    totalDays: breakdown.length,
    leaveDays,
    weekendDays,
    holidayDays,
    breakdown,
  });
}

/**
 * Format leave day result as a table string.
 */
export function formatLeaveDayTable(result: LeaveDayResult): string {
  const lines: string[] = [];

  lines.push(`Trip: ${result.startDate} to ${result.endDate} (${result.totalDays} days)`);
  lines.push(`Leave days required: ${result.leaveDays}`);
  lines.push(`Weekend days: ${result.weekendDays}`);
  lines.push(`Holiday days: ${result.holidayDays}`);
  lines.push('');
  lines.push('Breakdown:');
  lines.push('| Date | Day | Type | Leave |');
  lines.push('|------|-----|------|-------|');

  for (const day of result.breakdown) {
    const type = day.isMakeupWorkday
      ? '補班'
      : day.isHoliday
      ? day.holidayName || '假日'
      : day.isWeekend
      ? '週末'
      : '平日';
    const leave = day.requiresLeave ? '✓' : '';
    lines.push(`| ${day.date} | ${DAY_NAMES_ZH[day.dayOfWeek]} | ${type} | ${leave} |`);
  }

  return lines.join('\n');
}

/**
 * Compare multiple trip options.
 */
export interface TripOption {
  id: string;
  startDate: string;
  endDate: string;
  price: number;
  currency: string;
  description?: string;
}

export interface TripComparison {
  id: string;
  startDate: string;
  endDate: string;
  totalDays: number;
  leaveDays: number;
  price: number;
  currency: string;
  pricePerLeaveDay: number;
  description?: string;
}

export function compareTripOptions(
  options: TripOption[],
  calendar: HolidayCalendar
): Result<TripComparison[]> {
  const comparisons: TripComparison[] = [];

  for (const opt of options) {
    const leaveResult = calculateLeaveDays(opt.startDate, opt.endDate, calendar);
    if (!leaveResult.ok) {
      return Result.err(`Error calculating leave for ${opt.id}: ${leaveResult.error}`);
    }

    const leave = leaveResult.value;
    comparisons.push({
      id: opt.id,
      startDate: opt.startDate,
      endDate: opt.endDate,
      totalDays: leave.totalDays,
      leaveDays: leave.leaveDays,
      price: opt.price,
      currency: opt.currency,
      pricePerLeaveDay: leave.leaveDays > 0 ? opt.price / leave.leaveDays : opt.price,
      description: opt.description,
    });
  }

  // Sort by price per leave day (best value first)
  comparisons.sort((a, b) => a.pricePerLeaveDay - b.pricePerLeaveDay);

  return Result.ok(comparisons);
}

/**
 * Format trip comparisons as a table.
 */
export function formatComparisonTable(comparisons: TripComparison[]): string {
  const lines: string[] = [];

  lines.push('| Option | Dates | Days | Leave | Price | $/Leave |');
  lines.push('|--------|-------|:----:|:-----:|------:|--------:|');

  for (const c of comparisons) {
    const dates = `${c.startDate.slice(5)} - ${c.endDate.slice(5)}`;
    const priceStr = `${c.currency} ${c.price.toLocaleString()}`;
    const perLeaveStr = `${c.currency} ${Math.round(c.pricePerLeaveDay).toLocaleString()}`;
    lines.push(`| ${c.id} | ${dates} | ${c.totalDays} | ${c.leaveDays} | ${priceStr} | ${perLeaveStr} |`);
  }

  return lines.join('\n');
}

// CLI entry point
if (require.main === module) {
  const args = process.argv.slice(2);

  if (args.length < 2) {
    console.log('Usage: npx ts-node src/utils/leave-calculator.ts <start-date> <end-date> [calendar-path]');
    console.log('Example: npx ts-node src/utils/leave-calculator.ts 2026-02-24 2026-02-28 data/holidays/taiwan-2026.json');
    process.exit(1);
  }

  const [startDate, endDate, calendarPath = 'data/holidays/taiwan-2026.json'] = args;

  const calendarResult = loadHolidayCalendar(calendarPath);
  if (!calendarResult.ok) {
    console.error(calendarResult.error);
    process.exit(1);
  }

  const result = calculateLeaveDays(startDate, endDate, calendarResult.value);
  if (!result.ok) {
    console.error(result.error);
    process.exit(1);
  }

  console.log(formatLeaveDayTable(result.value));
}
