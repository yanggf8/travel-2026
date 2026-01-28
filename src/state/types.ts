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

// Event log state (in state.json)
export interface EventLogState {
  session: string;
  project: string;
  version: string;
  active_destination: string;
  current_focus: string;
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
