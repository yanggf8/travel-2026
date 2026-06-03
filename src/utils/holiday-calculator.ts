/**
 * Holiday Calculator — canonical module for holiday-aware date operations.
 *
 * Loads holiday calendars from Turso and caches them in memory.
 *
 * Usage:
 *   import { isHoliday, isWorkday, calculateLeave } from '../utils/holiday-calculator';
 *
 *   isHoliday('2026-02-15', 'tw');       // true  (除夕)
 *   isWorkday('2026-02-07', 'tw');        // true  (春節補班, Saturday)
 *   calculateLeave({ startDate: '2026-02-13', endDate: '2026-02-17', market: 'tw' });
 */

import {
  type HolidayCalendar,
  type HolidayEntry,
  type MakeupWorkday,
  type LeaveDayResult,
  type DayDetail,
  calculateLeaveDays,
} from '../utils/leave-calculator';
import { loadHolidayCalendarFromTurso } from '../services/turso-service';

// Re-export types that consumers may need
export type { HolidayCalendar, HolidayEntry, MakeupWorkday, LeaveDayResult, DayDetail };

// ---------------------------------------------------------------------------
// Market → country resolution
// ---------------------------------------------------------------------------

/** Map short market codes to country names used in calendar filenames */
const MARKET_TO_COUNTRY: Record<string, string> = {
  tw: 'taiwan',
  jp: 'japan',
};

/**
 * Resolve the Turso country key for a market code.
 */
function resolveCountry(market: string): string {
  return MARKET_TO_COUNTRY[market] ?? market;
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
export async function getCalendar(market: string, year: number): Promise<HolidayCalendar> {
  const key = cacheKey(market, year);
  const cached = calendarCache.get(key);
  if (cached) return cached;

  const calendar = await loadHolidayCalendarFromTurso(resolveCountry(market), year);
  calendarCache.set(key, calendar);
  return calendar;
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
export async function isHoliday(dateStr: string, market: string): Promise<boolean> {
  const cal = await getCalendar(market, yearOf(dateStr));
  return toMonthDay(dateStr) in cal.holidays;
}

/**
 * Get holiday info for a date, or null if it's not a holiday.
 */
export async function getHolidayInfo(dateStr: string, market: string): Promise<HolidayEntry | null> {
  const cal = await getCalendar(market, yearOf(dateStr));
  return cal.holidays[toMonthDay(dateStr)] ?? null;
}

/**
 * Check whether a date is a makeup workday (補班).
 */
export async function isMakeupWorkday(dateStr: string, market: string): Promise<boolean> {
  const cal = await getCalendar(market, yearOf(dateStr));
  return toMonthDay(dateStr) in cal.makeup_workdays;
}

/**
 * Get makeup workday info for a date, or null.
 */
export async function getMakeupWorkdayInfo(dateStr: string, market: string): Promise<MakeupWorkday | null> {
  const cal = await getCalendar(market, yearOf(dateStr));
  return cal.makeup_workdays[toMonthDay(dateStr)] ?? null;
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
export async function isWorkday(dateStr: string, market: string): Promise<boolean> {
  if (await isMakeupWorkday(dateStr, market)) return true;
  if (await isHoliday(dateStr, market)) return false;
  return !isWeekend(dateStr);
}

/**
 * A date requires leave if it's a workday (see above).
 */
export async function requiresLeave(dateStr: string, market: string): Promise<boolean> {
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
export async function getDateRange(startDate: string, endDate: string, market: string): Promise<DateInfo[]> {
  const start = parseDate(startDate);
  const end = parseDate(endDate);
  const results: DateInfo[] = [];
  const current = new Date(start);

  while (current <= end) {
    const y = current.getFullYear();
    const m = String(current.getMonth() + 1).padStart(2, '0');
    const d = String(current.getDate()).padStart(2, '0');
    const dateStr = `${y}-${m}-${d}`;

    const holiday = await getHolidayInfo(dateStr, market);
    const makeup = await isMakeupWorkday(dateStr, market);
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
export async function getHolidaysInRange(
  startDate: string,
  endDate: string,
  market: string
): Promise<Array<{ date: string; name: string; name_en: string; type: string }>> {
  const range = await getDateRange(startDate, endDate, market);
  return range
    .filter((d) => d.isHoliday)
    .map((d) => {
      const cal = calendarCache.get(cacheKey(market, yearOf(d.date)));
      const info = cal?.holidays[toMonthDay(d.date)]!;
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
export async function calculateLeave(opts: {
  startDate: string;
  endDate: string;
  market: string;
}): Promise<LeaveResult> {
  const year = yearOf(opts.startDate);
  const calendar = await getCalendar(opts.market, year);

  const result = calculateLeaveDays(opts.startDate, opts.endDate, calendar);
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
