/**
 * Holiday Calculator — canonical module for holiday-aware date operations.
 *
 * Loads holiday calendars from data/holidays/ via destinations.json origin config.
 * Caches loaded calendars in memory so repeated queries don't re-read disk.
 *
 * Usage:
 *   import { isHoliday, isWorkday, calculateLeave } from '../utilities/holiday-calculator';
 *
 *   isHoliday('2026-02-15', 'tw');       // true  (除夕)
 *   isWorkday('2026-02-07', 'tw');        // true  (春節補班, Saturday)
 *   calculateLeave({ startDate: '2026-02-13', endDate: '2026-02-17', market: 'tw' });
 */

import * as fs from 'fs';
import * as path from 'path';
import {
  type HolidayCalendar,
  type HolidayEntry,
  type MakeupWorkday,
  type LeaveDayResult,
  type DayDetail,
  calculateLeaveDays,
} from '../utils/leave-calculator';
import { Result } from '../types';

// Re-export types that consumers may need
export type { HolidayCalendar, HolidayEntry, MakeupWorkday, LeaveDayResult, DayDetail };

// ---------------------------------------------------------------------------
// Market → calendar path resolution
// ---------------------------------------------------------------------------

/** Map short market codes to country names used in calendar filenames */
const MARKET_TO_COUNTRY: Record<string, string> = {
  tw: 'taiwan',
  jp: 'japan',
};

interface OriginConfig {
  holiday_calendar: string;
}

interface DestinationsFile {
  origins: Record<string, OriginConfig>;
}

/**
 * Resolve the calendar file path for a market code.
 * Reads origins from destinations.json so the mapping stays in one place.
 */
function resolveCalendarPath(market: string, year: number): string {
  const country = MARKET_TO_COUNTRY[market] ?? market;

  // Try destinations.json first (authoritative)
  try {
    const destPath = path.resolve(process.cwd(), 'data/destinations.json');
    const raw = fs.readFileSync(destPath, 'utf-8');
    const dest: DestinationsFile = JSON.parse(raw);
    const origin = dest.origins[country];
    if (origin?.holiday_calendar) {
      // The path in destinations.json may contain a specific year;
      // replace if needed
      const calPath = origin.holiday_calendar.replace(/\d{4}/, String(year));
      return path.resolve(process.cwd(), calPath);
    }
  } catch {
    // Fall through to convention-based lookup
  }

  // Convention: data/holidays/{country}-{year}.json
  return path.resolve(process.cwd(), `data/holidays/${country}-${year}.json`);
}

// ---------------------------------------------------------------------------
// Calendar cache
// ---------------------------------------------------------------------------

const calendarCache = new Map<string, HolidayCalendar>();

function cacheKey(market: string, year: number): string {
  return `${market}:${year}`;
}

/**
 * Load (or retrieve from cache) a holiday calendar for the given market and year.
 */
export function getCalendar(market: string, year: number): Result<HolidayCalendar> {
  const key = cacheKey(market, year);
  const cached = calendarCache.get(key);
  if (cached) return Result.ok(cached);

  const filePath = resolveCalendarPath(market, year);
  try {
    const content = fs.readFileSync(filePath, 'utf-8');
    const calendar: HolidayCalendar = JSON.parse(content);
    calendarCache.set(key, calendar);
    return Result.ok(calendar);
  } catch (e) {
    return Result.err(
      `Holiday calendar not found for market="${market}" year=${year} (tried ${filePath}): ${
        e instanceof Error ? e.message : String(e)
      }`
    );
  }
}

/** Clear the in-memory calendar cache (useful for testing). */
export function clearCalendarCache(): void {
  calendarCache.clear();
}

// ---------------------------------------------------------------------------
// Date helpers
// ---------------------------------------------------------------------------

function parseDate(dateStr: string): Date {
  const [y, m, d] = dateStr.split('-').map(Number);
  return new Date(y, m - 1, d);
}

function toMonthDay(dateStr: string): string {
  // "2026-02-15" → "02-15"
  return dateStr.slice(5);
}

function yearOf(dateStr: string): number {
  return parseInt(dateStr.substring(0, 4), 10);
}

// ---------------------------------------------------------------------------
// Single-date queries
// ---------------------------------------------------------------------------

/**
 * Check whether a date is a public holiday.
 */
export function isHoliday(dateStr: string, market: string): boolean {
  const cal = getCalendar(market, yearOf(dateStr));
  if (!cal.ok) return false;
  return toMonthDay(dateStr) in cal.value.holidays;
}

/**
 * Get holiday info for a date, or null if it's not a holiday.
 */
export function getHolidayInfo(dateStr: string, market: string): HolidayEntry | null {
  const cal = getCalendar(market, yearOf(dateStr));
  if (!cal.ok) return null;
  return cal.value.holidays[toMonthDay(dateStr)] ?? null;
}

/**
 * Check whether a date is a makeup workday (補班).
 */
export function isMakeupWorkday(dateStr: string, market: string): boolean {
  const cal = getCalendar(market, yearOf(dateStr));
  if (!cal.ok) return false;
  return toMonthDay(dateStr) in cal.value.makeup_workdays;
}

/**
 * Get makeup workday info for a date, or null.
 */
export function getMakeupWorkdayInfo(dateStr: string, market: string): MakeupWorkday | null {
  const cal = getCalendar(market, yearOf(dateStr));
  if (!cal.ok) return null;
  return cal.value.makeup_workdays[toMonthDay(dateStr)] ?? null;
}

/**
 * Check whether a date falls on a weekend (Sat/Sun).
 */
export function isWeekend(dateStr: string): boolean {
  const d = parseDate(dateStr);
  const day = d.getDay();
  return day === 0 || day === 6;
}

/**
 * A date is a workday if:
 * - It's a weekday AND not a holiday, OR
 * - It's a makeup workday (even if weekend)
 */
export function isWorkday(dateStr: string, market: string): boolean {
  if (isMakeupWorkday(dateStr, market)) return true;
  if (isHoliday(dateStr, market)) return false;
  return !isWeekend(dateStr);
}

/**
 * A date requires leave if it's a workday (see above).
 */
export function requiresLeave(dateStr: string, market: string): boolean {
  return isWorkday(dateStr, market);
}

// ---------------------------------------------------------------------------
// Range queries
// ---------------------------------------------------------------------------

export interface DateInfo {
  date: string;
  dayOfWeek: number;
  isWeekend: boolean;
  isHoliday: boolean;
  isMakeupWorkday: boolean;
  holidayName: string | null;
  requiresLeave: boolean;
}

/**
 * Get holiday/workday info for every date in a range (inclusive).
 */
export function getDateRange(startDate: string, endDate: string, market: string): DateInfo[] {
  const start = parseDate(startDate);
  const end = parseDate(endDate);
  const results: DateInfo[] = [];
  const current = new Date(start);

  while (current <= end) {
    const y = current.getFullYear();
    const m = String(current.getMonth() + 1).padStart(2, '0');
    const d = String(current.getDate()).padStart(2, '0');
    const dateStr = `${y}-${m}-${d}`;

    const holiday = getHolidayInfo(dateStr, market);
    const makeup = isMakeupWorkday(dateStr, market);
    const weekend = current.getDay() === 0 || current.getDay() === 6;

    results.push({
      date: dateStr,
      dayOfWeek: current.getDay(),
      isWeekend: weekend,
      isHoliday: !!holiday,
      isMakeupWorkday: makeup,
      holidayName: holiday?.name ?? null,
      requiresLeave: makeup ? true : (!!holiday || weekend) ? false : true,
    });

    current.setDate(current.getDate() + 1);
  }

  return results;
}

/**
 * List only the holidays within a date range.
 */
export function getHolidaysInRange(
  startDate: string,
  endDate: string,
  market: string
): Array<{ date: string; name: string; name_en: string; type: string }> {
  return getDateRange(startDate, endDate, market)
    .filter((d) => d.isHoliday)
    .map((d) => {
      const info = getHolidayInfo(d.date, market)!;
      return { date: d.date, name: info.name, name_en: info.name_en, type: info.type };
    });
}

// ---------------------------------------------------------------------------
// Leave calculation (convenience wrapper)
// ---------------------------------------------------------------------------

export interface LeaveResult {
  leaveDaysNeeded: number;
  totalDays: number;
  weekendDays: number;
  holidayDays: number;
  breakdown: DayDetail[];
}

/**
 * Calculate leave days for a trip.
 * Loads the correct calendar based on market code.
 *
 * @param opts.startDate  Trip start (YYYY-MM-DD)
 * @param opts.endDate    Trip end (YYYY-MM-DD)
 * @param opts.market     Origin market code ('tw', 'jp', etc.)
 */
export function calculateLeave(opts: {
  startDate: string;
  endDate: string;
  market: string;
}): LeaveResult {
  const year = yearOf(opts.startDate);
  const calResult = getCalendar(opts.market, year);

  if (!calResult.ok) {
    // No calendar — fall back to weekday-only count
    const dates = getDateRange(opts.startDate, opts.endDate, opts.market);
    const weekdays = dates.filter((d) => !d.isWeekend).length;
    return {
      leaveDaysNeeded: weekdays,
      totalDays: dates.length,
      weekendDays: dates.filter((d) => d.isWeekend).length,
      holidayDays: 0,
      breakdown: [],
    };
  }

  const result = calculateLeaveDays(opts.startDate, opts.endDate, calResult.value);
  if (!result.ok) {
    return { leaveDaysNeeded: 0, totalDays: 0, weekendDays: 0, holidayDays: 0, breakdown: [] };
  }

  const v = result.value;
  return {
    leaveDaysNeeded: v.leaveDays,
    totalDays: v.totalDays,
    weekendDays: v.weekendDays,
    holidayDays: v.holidayDays,
    breakdown: v.breakdown,
  };
}
