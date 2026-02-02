/**
 * Itinerary Validator Types
 *
 * Types for validating travel itinerary reasonableness.
 */

/**
 * Validation issue severity
 */
export type IssueSeverity = 'error' | 'warning' | 'info';

/**
 * Validation issue category
 */
export type IssueCategory =
  | 'time_conflict'        // Overlapping activities
  | 'unrealistic_timing'   // Not enough time for activity
  | 'transport_gap'        // Missing transit time between areas
  | 'business_hours'       // Activity outside operating hours
  | 'booking_deadline'     // Booking deadline approaching or passed
  | 'capacity_risk'        // Popular venue may be sold out
  | 'missing_transport'    // No transport plan for day
  | 'budget_exceeded'      // Over budget
  | 'overcrowded_day'      // Too many activities
  | 'underpacked_day'      // Too few activities for the time available
  | 'logical_order'        // Activities in wrong order (e.g., dinner before lunch)
  | 'area_inefficiency';   // Back-and-forth between distant areas

/**
 * A single validation issue
 */
export interface ValidationIssue {
  severity: IssueSeverity;
  category: IssueCategory;
  day?: number;
  session?: 'morning' | 'afternoon' | 'evening';
  activityId?: string;
  message: string;
  suggestion?: string;
  autoFixable?: boolean;
}

/**
 * Validation result for an entire itinerary
 */
export interface ItineraryValidationResult {
  valid: boolean;
  issues: ValidationIssue[];
  summary: {
    errors: number;
    warnings: number;
    info: number;
  };
  validatedAt: string;
}

/**
 * Activity with resolved time info for validation
 */
export interface ResolvedActivity {
  id: string;
  title: string;
  day: number;
  session: 'morning' | 'afternoon' | 'evening';
  startTime?: string;  // HH:MM
  endTime?: string;    // HH:MM
  durationMin: number;
  isFixedTime: boolean;
  area?: string;
  bookingRequired: boolean;
  bookingStatus?: string;
  bookByDate?: string;
  operatingHours?: string;
}

/**
 * Day summary for validation
 */
export interface DaySummary {
  dayNumber: number;
  date: string;
  theme: string;
  activities: ResolvedActivity[];
  areas: string[];
  totalDurationMin: number;
  fixedTimeCount: number;
  pendingBookings: number;
}

/**
 * Validator options
 */
export interface ValidatorOptions {
  /**
   * Minimum transit time between areas (default: 30 min)
   */
  minTransitMinutes?: number;

  /**
   * Days before deadline to warn (default: 7)
   */
  bookingWarningDays?: number;

  /**
   * Max activities per session (default: 3)
   */
  maxActivitiesPerSession?: number;

  /**
   * Max total hours per day (default: 12)
   */
  maxHoursPerDay?: number;

  /**
   * Consider areas for efficiency check
   */
  checkAreaEfficiency?: boolean;

  /**
   * Check business hours against POI data
   */
  checkBusinessHours?: boolean;
}

/**
 * Area transit times lookup
 */
export interface TransitTimeLookup {
  /**
   * Get estimated transit time between two areas
   */
  getTransitMinutes(fromArea: string, toArea: string): number;
}
