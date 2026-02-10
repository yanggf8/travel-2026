/**
 * State Manager Types
 *
 * Shared types for travel plan state management.
 * Used by skills, cascade runner, and state manager.
 */

export const PROCESS_STATUSES = [
  'pending',
  'researching',
  'researched',
  'selecting',
  'selected',
  'populated', // Filled from package selection
  'booking',
  'booked',
  'confirmed',
  'skipped',
] as const;

// Process status values
export type ProcessStatus = (typeof PROCESS_STATUSES)[number];

const PROCESS_STATUS_SET: ReadonlySet<string> = new Set(PROCESS_STATUSES);

/** Runtime guard: returns true if the value is a valid ProcessStatus. */
export function isValidProcessStatus(value: unknown): value is ProcessStatus {
  return typeof value === 'string' && PROCESS_STATUS_SET.has(value);
}

// Valid status transitions
export const STATUS_TRANSITIONS: Record<ProcessStatus, ProcessStatus[]> = {
  pending: ['researching', 'populated', 'confirmed', 'skipped'],
  researching: ['researched', 'pending', 'skipped'],
  researched: ['selecting', 'selected', 'researching', 'skipped'],
  selecting: ['selected', 'researched', 'skipped'],
  selected: ['booking', 'selecting', 'populated', 'skipped'],
  populated: ['booking', 'selected', 'pending', 'skipped'],  // From package cascade
  booking: ['booked', 'selected', 'skipped'],
  booked: ['confirmed', 'skipped'],
  confirmed: ['skipped'],  // Can skip confirmed if plans change
  skipped: ['pending'],  // Can unskip back to pending
};

export const PROCESS_IDS = [
  'process_1_date_anchor',
  'process_2_destination',
  'process_3_4_packages',
  'process_3_transportation',
  'process_4_accommodation',
  'process_5_daily_itinerary',
] as const;

// Process identifiers
export type ProcessId = (typeof PROCESS_IDS)[number];

// Dirty flag state
export interface DirtyFlag {
  dirty: boolean;
  last_changed: string | null;
}

// Per-destination dirty flags
export interface DestinationDirtyFlags {
  [process: string]: DirtyFlag;
}

// Global dirty flags
export interface GlobalDirtyFlags {
  process_1_date_anchor: DirtyFlag;
  active_destination_last?: string;
}

// Full cascade state
export interface CascadeState {
  last_cascade_run: string;
  global: GlobalDirtyFlags;
  destinations: Record<string, DestinationDirtyFlags>;
}

// Travel event for audit log
export interface TravelEvent {
  event: string;
  at: string;
  from?: ProcessStatus;
  to?: ProcessStatus;
  destination?: string;
  process?: string;
  data?: Record<string, unknown>;
}

/**
 * Simplified state interface for testing and in-memory operation.
 * Subset of EventLogState with only the fields needed for StateManager.
 */
export interface TravelState {
  schema_version?: string;
  current_phase?: string;
  event_log: TravelEvent[];
  next_actions?: string[];
  dirty_flags?: Record<string, boolean>;
}

// Event log state (in state.json)
export interface EventLogState {
  session: string;
  project: string;
  version: string;
  active_destination: string;
  current_focus: string;
  next_actions?: string[];
  event_log: TravelEvent[];
  global_processes: Record<string, {
    state: ProcessStatus;
    events: TravelEvent[];
  }>;
  destinations: Record<string, {
    status: 'active' | 'archived';
    processes: Record<string, {
      state: ProcessStatus;
      events: TravelEvent[];
    }>;
  }>;
}

// Minimal travel plan shape for state manager
export interface TravelPlanMinimal {
  schema_version: string;
  active_destination: string;
  cascade_state: CascadeState;
  destinations: Record<string, {
    slug: string;
    status: string;
    [process: string]: unknown;
  }>;
}

// ============================================================================
// Activity Schema (P5 Itinerary)
// ============================================================================

export type SessionType = 'morning' | 'afternoon' | 'evening';

// Booking status for activities that require advance booking
export const BOOKING_STATUSES = [
  'not_required',  // No booking needed
  'pending',       // Needs to be booked
  'booked',        // Confirmed booking
  'waitlist',      // On waitlist
] as const;

export type BookingStatus = (typeof BOOKING_STATUSES)[number];

// ============================================================================
// Transportation (Ground Transfers)
// ============================================================================

export const TRANSFER_STATUSES = [
  'planned',
  'booked',
] as const;

export type TransferStatus = (typeof TRANSFER_STATUSES)[number];

export interface TransportOption {
  id: string;
  title: string;
  route: string;
  duration_min?: number | null;
  price_yen?: number | null;
  schedule?: string | null;
  booking_url?: string | null;
  notes?: string | null;
  tags?: string[];
}

export interface TransportSegment {
  status: TransferStatus;
  selected?: TransportOption | null;
  candidates: TransportOption[];
}

export interface AirportTransfers {
  arrival?: TransportSegment;
  departure?: TransportSegment;
}

/**
 * Canonical Activity schema for P5 itinerary.
 * All activities have IDs for CRUD operations.
 */
export interface Activity {
  id: string;                    // Unique ID: activity_{timestamp}_{random}
  title: string;                 // Display name
  area: string;                  // Neighborhood/district (e.g., "Shinjuku", "Asakusa")
  nearest_station: string | null; // Closest train station
  duration_min: number | null;   // Estimated duration in minutes
  booking_required: boolean;     // Needs advance booking?
  booking_url: string | null;    // Booking link if applicable
  booking_status?: BookingStatus; // Current booking state
  booking_ref?: string;          // Confirmation number/reference
  book_by?: string;              // Deadline to book (ISO date: YYYY-MM-DD)
  start_time?: string;           // Optional start time ("HH:MM")
  end_time?: string;             // Optional end time ("HH:MM")
  is_fixed_time?: boolean;       // True = hard constraint (reservation/flight)
  cost_estimate: number | null;  // Estimated cost in local currency
  tags: string[];                // Categorization: ["shopping", "food", "temple", "museum", etc.]
  notes: string | null;          // Free-form notes
  priority: 'must' | 'want' | 'optional'; // Priority level
}

/**
 * Session within a day (morning/afternoon/evening).
 */
export interface DaySession {
  focus: string | null;          // Theme for this session
  activities: Array<Activity | string>; // Ordered list of activities (legacy strings allowed)
  meals: string[];               // Meal suggestions
  transit_notes: string | null;  // Transit info (arrival/departure notes)
  booking_notes: string | null;  // Booking reminders
  time_range?: { start: string; end: string }; // Optional session boundary ("HH:MM")
}

/**
 * Weather forecast for a single day.
 */
export interface DayWeather {
  temp_high_c: number;
  temp_low_c: number;
  precipitation_pct: number;    // 0-100
  weather_code: number;         // WMO weather code
  weather_label: string;        // "Clear sky", "Partly cloudy", etc.
  source_id: string;            // "open_meteo"
  sourced_at: string;           // ISO-8601
}

/**
 * Single day in the itinerary.
 */
export interface ItineraryDay {
  date: string;                  // ISO date: YYYY-MM-DD
  day_number: number;            // 1-indexed day number
  day_type: 'arrival' | 'full' | 'departure';
  status: 'draft' | 'planned' | 'confirmed';
  theme: string | null;          // Day theme
  weather?: DayWeather;          // Optional weather forecast
  morning: DaySession;
  afternoon: DaySession;
  evening: DaySession;
}

/**
 * Generate unique activity ID.
 */
export function generateActivityId(): string {
  const timestamp = Date.now().toString(36);
  const random = Math.random().toString(36).substring(2, 6);
  return `activity_${timestamp}_${random}`;
}
