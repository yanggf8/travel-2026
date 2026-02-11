/**
 * State Commands
 *
 * Discriminated union of all state-mutating operations.
 * Each command maps 1:1 to an existing StateManager method.
 * This is the API contract for dispatch().
 */

import type {
  ProcessId,
  ProcessStatus,
  SessionType,
  DayWeather,
  BookingStatus,
  TransportOption,
  TransportSegment,
} from './types';

// ============================================================================
// Activity Input (shared across commands)
// ============================================================================

export interface ActivityInput {
  title: string;
  area?: string;
  nearest_station?: string;
  duration_min?: number;
  booking_required?: boolean;
  booking_url?: string;
  cost_estimate?: number;
  tags?: string[];
  notes?: string;
  priority?: 'must' | 'want' | 'optional';
}

export interface ActivityUpdateInput {
  title?: string;
  area?: string;
  nearest_station?: string;
  duration_min?: number;
  booking_required?: boolean;
  booking_url?: string;
  start_time?: string;
  end_time?: string;
  is_fixed_time?: boolean;
  cost_estimate?: number;
  tags?: string[];
  notes?: string;
  priority?: 'must' | 'want' | 'optional';
}

// ============================================================================
// Command Types
// ============================================================================

// --- Date Anchor ---
export interface SetDateAnchorCommand {
  type: 'set_date_anchor';
  startDate: string;
  endDate: string;
  reason?: string;
}

// --- Process Status ---
export interface SetProcessStatusCommand {
  type: 'set_process_status';
  destination: string;
  process: ProcessId;
  status: ProcessStatus;
}

// --- Dirty Flags ---
export interface MarkDirtyCommand {
  type: 'mark_dirty';
  destination: string;
  process: ProcessId;
}

export interface ClearDirtyCommand {
  type: 'clear_dirty';
  destination: string;
  process: ProcessId;
}

export interface MarkGlobalDirtyCommand {
  type: 'mark_global_dirty';
  process: 'process_1_date_anchor';
}

export interface ClearGlobalDirtyCommand {
  type: 'clear_global_dirty';
  process: 'process_1_date_anchor';
}

// --- Active Destination ---
export interface SetActiveDestinationCommand {
  type: 'set_active_destination';
  destination: string;
}

export interface SetFocusCommand {
  type: 'set_focus';
  destination: string;
  process: ProcessId;
}

export interface SetNextActionsCommand {
  type: 'set_next_actions';
  actions: string[];
}

export interface MarkCascadeRunCommand {
  type: 'mark_cascade_run';
  timestamp?: string;
}

// --- Offers ---
export interface UpdateOfferAvailabilityCommand {
  type: 'update_offer_availability';
  offerId: string;
  date: string;
  availability: 'available' | 'sold_out' | 'limited';
  price?: number;
  seatsRemaining?: number;
  source?: string;
}

export interface SelectOfferCommand {
  type: 'select_offer';
  offerId: string;
  date: string;
  populateCascade?: boolean;
}

export interface ImportPackageOffersCommand {
  type: 'import_package_offers';
  destination: string;
  sourceId: string;
  offers: Array<Record<string, unknown>>;
  note?: string;
  warnings?: string[];
}

// --- Transport ---
export interface SetAirportTransferCommand {
  type: 'set_airport_transfer';
  destination: string;
  direction: 'arrival' | 'departure';
  segment: TransportSegment;
}

export interface AddAirportTransferCandidateCommand {
  type: 'add_airport_transfer_candidate';
  destination: string;
  direction: 'arrival' | 'departure';
  option: TransportOption;
}

export interface SelectAirportTransferCommand {
  type: 'select_airport_transfer';
  destination: string;
  direction: 'arrival' | 'departure';
  optionId: string;
}

// --- Itinerary ---
export interface ScaffoldItineraryCommand {
  type: 'scaffold_itinerary';
  destination: string;
  days: Array<Record<string, unknown>>;
  force?: boolean;
}

export interface AddActivityCommand {
  type: 'add_activity';
  destination: string;
  dayNumber: number;
  session: SessionType;
  activity: ActivityInput;
}

export interface UpdateActivityCommand {
  type: 'update_activity';
  destination: string;
  dayNumber: number;
  session: SessionType;
  activityId: string;
  updates: ActivityUpdateInput;
}

export interface RemoveActivityCommand {
  type: 'remove_activity';
  destination: string;
  dayNumber: number;
  session: SessionType;
  activityId: string;
}

export interface SetActivityBookingCommand {
  type: 'set_activity_booking';
  destination: string;
  dayNumber: number;
  session: SessionType;
  activityIdOrTitle: string;
  status: BookingStatus;
  ref?: string;
  bookBy?: string;
}

export interface SetActivityTimeCommand {
  type: 'set_activity_time';
  destination: string;
  dayNumber: number;
  session: SessionType;
  activityIdOrTitle: string;
  startTime?: string;
  endTime?: string;
  isFixedTime?: boolean;
}

export interface SetSessionTimeRangeCommand {
  type: 'set_session_time_range';
  destination: string;
  dayNumber: number;
  session: SessionType;
  start: string;
  end: string;
}

export interface SetDayThemeCommand {
  type: 'set_day_theme';
  destination: string;
  dayNumber: number;
  theme: string | null;
}

export interface SetDayWeatherCommand {
  type: 'set_day_weather';
  destination: string;
  dayNumber: number;
  weather: DayWeather;
}

export interface SetSessionFocusCommand {
  type: 'set_session_focus';
  destination: string;
  dayNumber: number;
  session: SessionType;
  focus: string | null;
}

// ============================================================================
// Discriminated Union
// ============================================================================

export type Command =
  // Date anchor
  | SetDateAnchorCommand
  // Process status
  | SetProcessStatusCommand
  // Dirty flags
  | MarkDirtyCommand
  | ClearDirtyCommand
  | MarkGlobalDirtyCommand
  | ClearGlobalDirtyCommand
  // Destination & focus
  | SetActiveDestinationCommand
  | SetFocusCommand
  | SetNextActionsCommand
  | MarkCascadeRunCommand
  // Offers
  | UpdateOfferAvailabilityCommand
  | SelectOfferCommand
  | ImportPackageOffersCommand
  // Transport
  | SetAirportTransferCommand
  | AddAirportTransferCandidateCommand
  | SelectAirportTransferCommand
  // Itinerary
  | ScaffoldItineraryCommand
  | AddActivityCommand
  | UpdateActivityCommand
  | RemoveActivityCommand
  | SetActivityBookingCommand
  | SetActivityTimeCommand
  | SetSessionTimeRangeCommand
  | SetDayThemeCommand
  | SetDayWeatherCommand
  | SetSessionFocusCommand;

/** Result from dispatch — includes any generated IDs or return values. */
export interface DispatchResult {
  /** Generated activity ID (for add_activity command). */
  activityId?: string;
}
