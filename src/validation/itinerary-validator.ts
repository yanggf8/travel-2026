/**
 * Itinerary Validator
 *
 * Validates travel itinerary for:
 * - Time conflicts between activities
 * - Realistic timing and transit
 * - Business hours compliance
 * - Booking deadlines
 * - Day packing (over/under)
 * - Area efficiency
 */

import {
  ValidationIssue,
  ItineraryValidationResult,
  ResolvedActivity,
  DaySummary,
  ValidatorOptions,
  IssueSeverity,
  IssueCategory,
} from './types';

const DEFAULT_OPTIONS: Required<ValidatorOptions> = {
  minTransitMinutes: 30,
  bookingWarningDays: 7,
  maxActivitiesPerSession: 3,
  maxHoursPerDay: 12,
  checkAreaEfficiency: true,
  checkBusinessHours: true,
};

/**
 * Parse time string "HH:MM" to minutes from midnight
 */
function parseTimeToMinutes(time: string): number {
  const [hours, minutes] = time.split(':').map(Number);
  return hours * 60 + minutes;
}

/**
 * Format minutes to "HH:MM"
 */
function formatMinutesToTime(minutes: number): string {
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  return `${h.toString().padStart(2, '0')}:${m.toString().padStart(2, '0')}`;
}

/**
 * Check if two time ranges overlap
 */
function timesOverlap(
  start1: number,
  end1: number,
  start2: number,
  end2: number
): boolean {
  return start1 < end2 && start2 < end1;
}

/**
 * Parse operating hours like "11:00-20:00" or "24h"
 */
function parseOperatingHours(hours: string | undefined): { open: number; close: number } | null {
  if (!hours || hours === '24h' || hours.toLowerCase() === 'always') {
    return null; // Always open
  }

  const match = hours.match(/(\d{1,2}:\d{2})\s*[-–]\s*(\d{1,2}:\d{2})/);
  if (match) {
    return {
      open: parseTimeToMinutes(match[1]),
      close: parseTimeToMinutes(match[2]),
    };
  }

  return null;
}

/**
 * Default session time ranges
 */
const SESSION_DEFAULTS = {
  morning: { start: 9 * 60, end: 12 * 60 },      // 09:00-12:00
  afternoon: { start: 12 * 60, end: 18 * 60 },   // 12:00-18:00
  evening: { start: 18 * 60, end: 22 * 60 },     // 18:00-22:00
};

export class ItineraryValidator {
  private options: Required<ValidatorOptions>;

  constructor(options?: ValidatorOptions) {
    this.options = { ...DEFAULT_OPTIONS, ...options };
  }

  /**
   * Validate an entire itinerary
   */
  validate(days: DaySummary[], today?: Date): ItineraryValidationResult {
    const issues: ValidationIssue[] = [];
    const currentDate = today || new Date();

    for (const day of days) {
      // Per-day validations
      issues.push(...this.validateDayConflicts(day));
      issues.push(...this.validateDayPacking(day));
      issues.push(...this.validateBookingDeadlines(day, currentDate));

      if (this.options.checkBusinessHours) {
        issues.push(...this.validateBusinessHours(day));
      }

      if (this.options.checkAreaEfficiency) {
        issues.push(...this.validateAreaEfficiency(day));
      }
    }

    // Cross-day validations
    issues.push(...this.validateLogicalOrder(days));

    const summary = {
      errors: issues.filter((i) => i.severity === 'error').length,
      warnings: issues.filter((i) => i.severity === 'warning').length,
      info: issues.filter((i) => i.severity === 'info').length,
    };

    return {
      valid: summary.errors === 0,
      issues,
      summary,
      validatedAt: new Date().toISOString(),
    };
  }

  /**
   * Check for time conflicts within a day
   */
  private validateDayConflicts(day: DaySummary): ValidationIssue[] {
    const issues: ValidationIssue[] = [];
    const activitiesWithTimes = day.activities.filter(
      (a) => a.startTime && a.endTime
    );

    for (let i = 0; i < activitiesWithTimes.length; i++) {
      const a1 = activitiesWithTimes[i];
      const start1 = parseTimeToMinutes(a1.startTime!);
      const end1 = parseTimeToMinutes(a1.endTime!);

      for (let j = i + 1; j < activitiesWithTimes.length; j++) {
        const a2 = activitiesWithTimes[j];
        const start2 = parseTimeToMinutes(a2.startTime!);
        const end2 = parseTimeToMinutes(a2.endTime!);

        if (timesOverlap(start1, end1, start2, end2)) {
          issues.push({
            severity: 'error',
            category: 'time_conflict',
            day: day.dayNumber,
            message: `Time conflict: "${a1.title}" (${a1.startTime}-${a1.endTime}) overlaps with "${a2.title}" (${a2.startTime}-${a2.endTime})`,
            suggestion: `Adjust timing for one of these activities`,
            autoFixable: false,
          });
        }
      }
    }

    return issues;
  }

  /**
   * Check if day is over/under packed
   */
  private validateDayPacking(day: DaySummary): ValidationIssue[] {
    const issues: ValidationIssue[] = [];
    const maxMinutes = this.options.maxHoursPerDay * 60;

    // Check total duration
    if (day.totalDurationMin > maxMinutes) {
      issues.push({
        severity: 'warning',
        category: 'overcrowded_day',
        day: day.dayNumber,
        message: `Day ${day.dayNumber} has ${Math.round(day.totalDurationMin / 60)} hours of activities (max: ${this.options.maxHoursPerDay}h)`,
        suggestion: 'Consider moving some activities to another day',
      });
    }

    // Check per-session activity count
    const bySession = new Map<string, number>();
    for (const a of day.activities) {
      const count = bySession.get(a.session) || 0;
      bySession.set(a.session, count + 1);
    }

    for (const [session, count] of bySession) {
      if (count > this.options.maxActivitiesPerSession) {
        issues.push({
          severity: 'warning',
          category: 'overcrowded_day',
          day: day.dayNumber,
          session: session as 'morning' | 'afternoon' | 'evening',
          message: `${session} has ${count} activities (max: ${this.options.maxActivitiesPerSession})`,
          suggestion: 'Spread activities across sessions',
        });
      }
    }

    // Check for underpacked days (excluding arrival/departure)
    const isArrivalOrDeparture = 
      day.theme?.toLowerCase().includes('arrival') ||
      day.theme?.toLowerCase().includes('departure');

    if (!isArrivalOrDeparture && day.activities.length === 0) {
      issues.push({
        severity: 'info',
        category: 'underpacked_day',
        day: day.dayNumber,
        message: `Day ${day.dayNumber} has no activities planned`,
        suggestion: 'Add activities or mark as rest day',
      });
    }

    return issues;
  }

  /**
   * Check booking deadlines
   */
  private validateBookingDeadlines(day: DaySummary, today: Date): ValidationIssue[] {
    const issues: ValidationIssue[] = [];

    for (const activity of day.activities) {
      if (!activity.bookingRequired) continue;

      const status = activity.bookingStatus || 'pending';
      if (status === 'booked' || status === 'not_required') continue;

      // Check if deadline is approaching or passed
      if (activity.bookByDate) {
        const deadline = new Date(activity.bookByDate);
        const daysUntil = Math.ceil((deadline.getTime() - today.getTime()) / (1000 * 60 * 60 * 24));

        if (daysUntil < 0) {
          issues.push({
            severity: 'error',
            category: 'booking_deadline',
            day: day.dayNumber,
            activityId: activity.id,
            message: `Booking deadline PASSED for "${activity.title}" (was ${activity.bookByDate})`,
            suggestion: 'Book immediately or remove activity',
          });
        } else if (daysUntil <= this.options.bookingWarningDays) {
          issues.push({
            severity: 'warning',
            category: 'booking_deadline',
            day: day.dayNumber,
            activityId: activity.id,
            message: `Booking deadline in ${daysUntil} day(s) for "${activity.title}" (${activity.bookByDate})`,
            suggestion: 'Complete booking soon',
          });
        }
      } else if (status === 'pending') {
        // No deadline set but booking required
        issues.push({
          severity: 'info',
          category: 'booking_deadline',
          day: day.dayNumber,
          activityId: activity.id,
          message: `"${activity.title}" requires booking but has no deadline set`,
          suggestion: 'Set a book_by date to track deadline',
        });
      }
    }

    return issues;
  }

  /**
   * Check business hours compliance
   */
  private validateBusinessHours(day: DaySummary): ValidationIssue[] {
    const issues: ValidationIssue[] = [];

    for (const activity of day.activities) {
      if (!activity.startTime || !activity.operatingHours) continue;

      const hours = parseOperatingHours(activity.operatingHours);
      if (!hours) continue; // Always open

      const activityStart = parseTimeToMinutes(activity.startTime);
      const activityEnd = activity.endTime
        ? parseTimeToMinutes(activity.endTime)
        : activityStart + activity.durationMin;

      if (activityStart < hours.open) {
        issues.push({
          severity: 'warning',
          category: 'business_hours',
          day: day.dayNumber,
          activityId: activity.id,
          message: `"${activity.title}" starts at ${activity.startTime} but opens at ${formatMinutesToTime(hours.open)}`,
          suggestion: `Start at or after ${formatMinutesToTime(hours.open)}`,
          autoFixable: true,
        });
      }

      if (activityEnd > hours.close) {
        issues.push({
          severity: 'warning',
          category: 'business_hours',
          day: day.dayNumber,
          activityId: activity.id,
          message: `"${activity.title}" ends at ${formatMinutesToTime(activityEnd)} but closes at ${formatMinutesToTime(hours.close)}`,
          suggestion: `Plan to finish by ${formatMinutesToTime(hours.close)}`,
          autoFixable: true,
        });
      }
    }

    return issues;
  }

  /**
   * Check area efficiency (minimize back-and-forth)
   */
  private validateAreaEfficiency(day: DaySummary): ValidationIssue[] {
    const issues: ValidationIssue[] = [];

    if (day.activities.length < 3) return issues;

    // Get sequence of areas (skip undefined)
    const areaSequence = day.activities
      .filter((a) => a.area)
      .map((a) => a.area!);

    if (areaSequence.length < 3) return issues;

    // Check for A -> B -> A pattern (back-and-forth)
    for (let i = 0; i < areaSequence.length - 2; i++) {
      if (
        areaSequence[i] === areaSequence[i + 2] &&
        areaSequence[i] !== areaSequence[i + 1]
      ) {
        issues.push({
          severity: 'info',
          category: 'area_inefficiency',
          day: day.dayNumber,
          message: `Day ${day.dayNumber} has back-and-forth travel: ${areaSequence[i]} → ${areaSequence[i + 1]} → ${areaSequence[i + 2]}`,
          suggestion: 'Reorder activities to minimize transit',
        });
        break; // Only report once per day
      }
    }

    // Count unique areas
    const uniqueAreas = new Set(areaSequence);
    if (uniqueAreas.size > 4) {
      issues.push({
        severity: 'info',
        category: 'area_inefficiency',
        day: day.dayNumber,
        message: `Day ${day.dayNumber} covers ${uniqueAreas.size} different areas`,
        suggestion: 'Consider clustering activities by area',
      });
    }

    return issues;
  }

  /**
   * Check logical order across days (e.g., not scheduling dinner before lunch)
   */
  private validateLogicalOrder(days: DaySummary[]): ValidationIssue[] {
    const issues: ValidationIssue[] = [];

    // Check for evening activities before morning on same day
    for (const day of days) {
      const sessions = ['morning', 'afternoon', 'evening'] as const;
      const sessionOrder = new Map(sessions.map((s, i) => [s, i]));

      const sortedByTime = [...day.activities]
        .filter((a) => a.startTime)
        .sort((a, b) => 
          parseTimeToMinutes(a.startTime!) - parseTimeToMinutes(b.startTime!)
        );

      for (let i = 0; i < sortedByTime.length - 1; i++) {
        const curr = sortedByTime[i];
        const next = sortedByTime[i + 1];

        const currSessionOrder = sessionOrder.get(curr.session) ?? 0;
        const nextSessionOrder = sessionOrder.get(next.session) ?? 0;

        if (currSessionOrder > nextSessionOrder) {
          issues.push({
            severity: 'warning',
            category: 'logical_order',
            day: day.dayNumber,
            message: `"${curr.title}" (${curr.session}) is scheduled before "${next.title}" (${next.session}) but has later time`,
            suggestion: 'Verify session assignments match actual times',
          });
        }
      }
    }

    return issues;
  }
}

export const defaultValidator = new ItineraryValidator();
