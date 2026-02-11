/**
 * State Repository Interface
 *
 * Abstracts plan data access from storage format.
 * Phase 0: BlobBridgeRepository implements this over the JSON blob.
 * Phase 2: TursoRepository implements this over normalized tables.
 *
 * All JSON path navigation and `Record<string, unknown>` casts
 * live in the repository implementation, not in StateManager.
 */

import type {
  ProcessId,
  ProcessStatus,
  SessionType,
  DayWeather,
  CascadeState,
  TravelEvent,
  TravelPlanMinimal,
  EventLogState,
  TransportOption,
  TransportSegment,
} from './types';

// ============================================================================
// Row Types (typed views of plan sub-structures)
// ============================================================================

export interface DateAnchorData {
  start: string;
  end: string;
  days: number;
}

export interface ActivitySearchResult {
  dayNumber: number;
  session: SessionType;
  activity: string | Record<string, unknown>;
  isString: boolean;
}

// ============================================================================
// StateReader — read-only access to plan data
// ============================================================================

export interface StateReader {
  // --- Plan metadata ---
  getActiveDestination(): string;
  getSchemaVersion(): string;

  // --- Process status ---
  getProcessStatus(dest: string, process: ProcessId): ProcessStatus | null;

  // --- Date anchor ---
  getDateAnchor(dest: string): DateAnchorData | null;

  // --- Cascade state ---
  getCascadeState(): CascadeState;
  isDirty(dest: string, process: ProcessId): boolean;

  // --- Itinerary ---
  getDay(dest: string, dayNumber: number): Record<string, unknown> | null;
  getDays(dest: string): Array<Record<string, unknown>>;
  getSessionActivities(dest: string, dayNumber: number, session: SessionType): Array<string | Record<string, unknown>> | null;
  findActivityIndex(activities: Array<string | Record<string, unknown>>, idOrTitle: string): number;
  findActivity(dest: string, idOrTitle: string): ActivitySearchResult | null;

  // --- Offers ---
  getOffers(dest: string): Array<Record<string, unknown>> | null;
  getOffer(dest: string, offerId: string): Record<string, unknown> | null;

  // --- Event log ---
  getEvents(): TravelEvent[];
  getNextActions(): string[];

  // --- Raw plan access (for cascade runner, validation, etc.) ---
  getPlan(): TravelPlanMinimal;
  getEventLog(): EventLogState;
}

// ============================================================================
// StateWriter — mutating operations on plan data
// ============================================================================

export interface StateWriter {
  // --- Plan metadata ---
  setActiveDestination(dest: string): void;

  // --- Process status ---
  /**
   * Set process status on the plan object (no validation — SM handles that).
   * Updates both plan JSON and event log state.
   */
  setProcessStatusData(dest: string, process: ProcessId, status: ProcessStatus, timestamp: string): void;

  // --- Date anchor ---
  setDateAnchorData(dest: string, start: string, end: string, days: number, timestamp: string): void;

  // --- Cascade state ---
  setDirtyFlag(dest: string, process: ProcessId, dirty: boolean, timestamp: string): void;
  setGlobalDirtyFlag(process: 'process_1_date_anchor', dirty: boolean, timestamp: string): void;
  markCascadeRun(timestamp: string): void;

  // --- Itinerary scaffolding ---
  setDays(dest: string, days: Array<Record<string, unknown>>, timestamp: string): void;
  touchItinerary(dest: string, timestamp: string): void;

  // --- Day-level mutations ---
  setDayField(dest: string, dayNumber: number, field: string, value: unknown): void;

  // --- Session-level mutations ---
  setSessionField(dest: string, dayNumber: number, session: SessionType, field: string, value: unknown): void;

  // --- Activity mutations ---
  /**
   * Push an activity to a session. Returns void; caller generates the ID.
   */
  addActivityToSession(dest: string, dayNumber: number, session: SessionType, activity: Record<string, unknown>): void;

  /**
   * Update activity at given index. Merges updates into existing object.
   * If the activity is a legacy string, upgrades it first.
   * Returns the (possibly upgraded) activity object.
   */
  updateActivityAtIndex(
    dest: string, dayNumber: number, session: SessionType, index: number,
    updates: Record<string, unknown>
  ): Record<string, unknown>;

  /**
   * Replace activity at given index with a new object.
   * Used when upgrading string activities.
   */
  replaceActivityAtIndex(dest: string, dayNumber: number, session: SessionType, index: number, activity: Record<string, unknown>): void;

  /**
   * Remove activity at index. Returns the removed item.
   */
  removeActivityAtIndex(dest: string, dayNumber: number, session: SessionType, index: number): string | Record<string, unknown>;

  // --- Offer mutations ---
  setOfferAvailability(dest: string, offerId: string, date: string, data: Record<string, unknown>): { previousAvailability: unknown };
  setOfferSelection(dest: string, offerId: string, date: string, timestamp: string): Record<string, unknown>;
  importOffers(dest: string, sourceId: string, offers: Array<Record<string, unknown>>, timestamp: string, note?: string, warnings?: string[]): void;
  populateFromOffer(dest: string, offer: Record<string, unknown>, date: string, timestamp: string): void;

  // --- Transport mutations ---
  ensureTransportationProcess(dest: string, timestamp: string): void;
  setAirportTransfer(dest: string, direction: 'arrival' | 'departure', segment: unknown, timestamp: string): void;
  addAirportTransferCandidate(dest: string, direction: 'arrival' | 'departure', option: TransportOption, timestamp: string): void;
  selectAirportTransferOption(dest: string, direction: 'arrival' | 'departure', optionId: string, timestamp: string): TransportOption;
  touchTransportation(dest: string, timestamp: string): void;

  // --- Event log mutations ---
  pushEvent(event: TravelEvent): void;
  ensureEventLogDestination(dest: string): void;
  setEventLogProcessState(dest: string, process: ProcessId, state: ProcessStatus): void;
  setEventLogActiveDestination(dest: string): void;
  setEventLogFocus(focus: string): void;
  setNextActions(actions: string[]): void;

  // --- Persistence ---
  save(planId: string, schemaVersion: string): Promise<void>;
}

// ============================================================================
// StateRepository — full interface
// ============================================================================

export interface StateRepository extends StateReader, StateWriter {}
