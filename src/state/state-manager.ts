/**
 * State Manager
 *
 * Unified state management for travel planning skills.
 * Handles dirty flags, process status, and event logging.
 *
 * Usage:
 *   import { StateManager } from './state-manager';
 *   const sm = new StateManager();
 *   sm.markDirty('tokyo_2026', 'process_3_transportation');
 *   sm.setProcessStatus('tokyo_2026', 'process_3_4_packages', 'selected');
 *   sm.save();
 */

import { readFileSync, existsSync, realpathSync } from 'fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {
  ProcessStatus,
  ProcessId,
  DirtyFlag,
  CascadeState,
  TravelEvent,
  EventLogState,
  TravelPlanMinimal,
  TravelState,
  STATUS_TRANSITIONS,
  TransportOption,
  TransportSegment,
} from './types';
import {
  validateTravelPlan,
  validateEventLogState,
  TravelPlan,
  EventLogState as ZodEventLogState,
} from './schemas';
import { DEFAULTS } from '../config/constants';
import { OfferManager } from './offer-manager';
import { TransportManager } from './transport-manager';
import { ItineraryManager } from './itinerary-manager';
import { EventQuery } from './event-query';

// Default paths
const DEFAULT_PLAN_PATH = process.env.TRAVEL_PLAN_PATH || 'data/travel-plan.json';
const DEFAULT_STATE_PATH = process.env.TRAVEL_STATE_PATH || 'data/state.json';

/**
 * Options for StateManager constructor.
 * Supports both file-based and in-memory operation (for testing).
 */
export interface StateManagerOptions {
  /** Path to travel-plan.json (default: data/travel-plan.json) */
  planPath?: string;
  /** Path to state.json (default: data/state.json) */
  statePath?: string;
  /** In-memory plan data (bypasses file loading) */
  plan?: TravelPlanMinimal;
  /** In-memory state data (bypasses file loading) */
  state?: TravelState;
  /** Full EventLogState from DB (bypasses reconstruction from TravelState) */
  eventLog?: EventLogState;
  /** Skip save operations (useful for tests) */
  skipSave?: boolean;
}

export class StateManager {
  private planPath: string;
  private statePath: string;
  private planId: string;

  // Domain managers
  private offerMgr: OfferManager;
  private transportMgr: TransportManager;
  private itineraryMgr: ItineraryManager;
  public events: EventQuery;
  private plan: TravelPlanMinimal;
  private eventLog: EventLogState;
  private timestamp: string;
  private skipSave: boolean;

  constructor(options?: StateManagerOptions | string, statePath?: string) {
    // Support legacy (planPath, statePath) signature
    if (typeof options === 'string' || options === undefined) {
      this.planPath = options || DEFAULT_PLAN_PATH;
      this.statePath = statePath || DEFAULT_STATE_PATH;
      this.planId = StateManager.derivePlanId(this.planPath);
      this.skipSave = false;
      this.timestamp = this.freshTimestamp();
      this.plan = this.loadPlan();
      this.normalizePlan();
      this.eventLog = this.loadEventLog();
    } else {
      // New options-based signature
      this.planPath = options.planPath || DEFAULT_PLAN_PATH;
      this.statePath = options.statePath || DEFAULT_STATE_PATH;
      this.planId = StateManager.derivePlanId(this.planPath);
      this.skipSave = options.skipSave || false;
      this.timestamp = this.freshTimestamp();

      if (options.plan) {
        this.plan = options.plan;
        this.normalizePlan();
      } else {
        this.plan = this.loadPlan();
        this.normalizePlan();
      }

      if (options.eventLog) {
        // Full EventLogState from DB — use directly, no reconstruction needed
        this.eventLog = options.eventLog;
      } else if (options.state) {
        // Create full EventLogState from minimal TravelState (tests)
        this.eventLog = {
          session: new Date().toISOString().split('T')[0],
          project: DEFAULTS.project,
          version: '3.0',
          active_destination: this.plan?.active_destination || '',
          current_focus: '',
          event_log: options.state.event_log || [],
          next_actions: options.state.next_actions || [],
          global_processes: {},
          destinations: {},
        };
      } else {
        this.eventLog = this.loadEventLog();
      }
    }

    // Initialize domain managers
    this.offerMgr = new OfferManager(
      this.plan,
      () => this.timestamp,
      (e) => this.emitEvent(e),
      (d, p, s) => this.setProcessStatus(d, p, s),
      (d, p) => this.clearDirty(d, p)
    );
    this.transportMgr = new TransportManager(
      this.plan,
      () => this.timestamp,
      (e) => this.emitEvent(e)
    );
    this.itineraryMgr = new ItineraryManager(
      this.plan,
      () => this.timestamp,
      (e) => this.emitEvent(e),
      (d, p, s) => this.setProcessStatus(d, p, s),
      (d, p, s, data) => this.forceSetProcessStatus(d, p, s, data),
      (d, p) => this.clearDirty(d, p)
    );
    this.events = new EventQuery(() => this.eventLog.event_log);
  }

  // ============================================================================
  // Factory (DB-primary)
  // ============================================================================

  /**
   * Derive a plan ID from the file path.
   * data/travel-plan.json → "default"
   * data/trips/<id>/travel-plan.json → "<id>"
   * Other paths → "path:<sha1-prefix>" to avoid cross-plan collision
   */
  static derivePlanId(planPath: string): string {
    const normalize = (p: string): string => p.replace(/\\/g, '/');
    const canonicalAbs = (p: string): string => {
      const resolved = path.resolve(p);
      try {
        return normalize(realpathSync(resolved));
      } catch {
        return normalize(resolved);
      }
    };

    const canonicalPath = canonicalAbs(planPath);
    const relFromRoot = normalize(path.relative(canonicalAbs(process.cwd()), canonicalPath));

    const tripsMatch = relFromRoot.match(/^data\/trips\/([^/]+)\//);
    if (tripsMatch) return tripsMatch[1];
    if (relFromRoot === 'data/travel-plan.json') {
      return 'default';
    }

    const hash = crypto.createHash('sha1').update(canonicalPath).digest('hex').slice(0, 12);
    return `path:${hash}`;
  }

  /**
   * DB-only factory: read plan+state from Turso.
   */
  static async create(
    planPathOrOpts?: string | StateManagerOptions,
    statePath?: string
  ): Promise<StateManager> {
    const planPath = typeof planPathOrOpts === 'string'
      ? planPathOrOpts
      : planPathOrOpts?.planPath || DEFAULT_PLAN_PATH;
    const stPath = typeof planPathOrOpts === 'string'
      ? (statePath || DEFAULT_STATE_PATH)
      : planPathOrOpts?.statePath || DEFAULT_STATE_PATH;
    const skipSave = typeof planPathOrOpts === 'object' ? planPathOrOpts?.skipSave || false : false;

    // If skipSave (test mode), skip DB entirely
    if (skipSave) {
      return new StateManager(
        typeof planPathOrOpts === 'object' ? planPathOrOpts : planPath,
        typeof planPathOrOpts === 'string' ? statePath : undefined
      );
    }

    const planId = StateManager.derivePlanId(planPath);

    const { readPlanFromDb } = require('../services/turso-service');

    let dbRow: { plan_json: string; state_json: string | null; updated_at: string } | null;
    try {
      dbRow = await readPlanFromDb(planId);
    } catch (e: any) {
      throw new Error(`[turso] DB read failed for plan "${planId}": ${e.message}`);
    }

    if (!dbRow) {
      throw new Error(`[turso] Plan "${planId}" not found in DB. Run 'npm run db:seed:plans' first.`);
    }

    let plan: TravelPlanMinimal;
    let fullEventLog: EventLogState | undefined;
    try {
      plan = JSON.parse(dbRow.plan_json) as TravelPlanMinimal;
      fullEventLog = dbRow.state_json ? JSON.parse(dbRow.state_json) as EventLogState : undefined;
    } catch (e: any) {
      throw new Error(`[turso] Invalid JSON in plans_current for plan "${planId}": ${e.message}`);
    }

    console.error(`  [turso] Loaded plan "${planId}" from DB (updated: ${dbRow.updated_at})`);

    return new StateManager({
      planPath: planPath,
      statePath: stPath,
      plan,
      eventLog: fullEventLog,
      skipSave: false,
    });
  }

  // ============================================================================
  // Timestamp
  // ============================================================================

  /**
   * Get atomic ISO timestamp for current session.
   * All updates in a single StateManager session use the same timestamp.
   */
  now(): string {
    return this.timestamp;
  }

  /**
   * Generate a fresh ISO timestamp (for new sessions/batches).
   */
  freshTimestamp(): string {
    return new Date().toISOString();
  }

  /**
   * Refresh session timestamp for new batch of operations.
   */
  refreshTimestamp(): void {
    this.timestamp = this.freshTimestamp();
  }

  // ============================================================================
  // Cascade Run Tracking
  // ============================================================================

  /**
   * Mark that a cascade run has completed.
   * Only the cascade runner should call this.
   * @param timestamp - Use cascade plan's computed_at for exact match
   */
  markCascadeRun(timestamp?: string): void {
    this.plan.cascade_state.last_cascade_run = timestamp || this.timestamp;
  }

  /**
   * Get the timestamp of the last cascade run.
   */
  getLastCascadeRun(): string {
    return this.plan.cascade_state.last_cascade_run;
  }

  // ============================================================================
  // Dirty Flags
  // ============================================================================

  /**
   * Mark a process as dirty (needs cascade re-evaluation).
   */
  markDirty(destination: string, process: ProcessId): void {
    this.ensureDestinationState(destination);
    this.plan.cascade_state.destinations[destination][process] = {
      dirty: true,
      last_changed: this.timestamp,
    };
    this.emitEvent({
      event: 'marked_dirty',
      destination,
      process,
      data: { dirty: true },
    });
  }

  /**
   * Clear dirty flag (after cascade processes it).
   */
  clearDirty(destination: string, process: ProcessId): void {
    this.ensureDestinationState(destination);
    const current = this.plan.cascade_state.destinations[destination][process];
    if (current) {
      current.dirty = false;
      current.last_changed = this.timestamp;
    }
  }

  /**
   * Mark global process as dirty.
   */
  markGlobalDirty(process: 'process_1_date_anchor'): void {
    this.plan.cascade_state.global[process] = {
      dirty: true,
      last_changed: this.timestamp,
    };
    this.emitEvent({
      event: 'marked_global_dirty',
      process,
      data: { dirty: true },
    });
  }

  /**
   * Clear global dirty flag.
   */
  clearGlobalDirty(process: 'process_1_date_anchor'): void {
    const current = this.plan.cascade_state.global[process];
    if (current) {
      current.dirty = false;
      current.last_changed = this.timestamp;
    }
  }

  /**
   * Get all dirty flags.
   */
  getDirtyFlags(): CascadeState {
    return this.plan.cascade_state;
  }

  /**
   * Check if a specific process is dirty.
   */
  isDirty(destination: string, process: ProcessId): boolean {
    const destState = this.plan.cascade_state.destinations[destination];
    return destState?.[process]?.dirty ?? false;
  }

  // ============================================================================
  // Process Status
  // ============================================================================

  /**
   * Set process status with transition validation.
   */
  setProcessStatus(
    destination: string,
    process: ProcessId,
    newStatus: ProcessStatus
  ): void {
    const currentStatus = this.getProcessStatus(destination, process);

    // Idempotent: allow setting the same status without emitting events.
    if (currentStatus === newStatus) {
      // Update in travel-plan.json (process object)
      const dest = this.plan.destinations[destination];
      if (dest && dest[process]) {
        const processObj = dest[process] as Record<string, unknown>;
        processObj['status'] = newStatus;
        processObj['updated_at'] = this.timestamp;
      }

      // Update in state.json
      this.ensureEventLogDestination(destination);
      const destLog = this.eventLog.destinations[destination];
      if (!destLog.processes[process]) {
        destLog.processes[process] = { state: newStatus, events: [] };
      }
      destLog.processes[process].state = newStatus;
      return;
    }

    // Validate transition
    if (currentStatus && !this.isValidTransition(currentStatus, newStatus)) {
      throw new Error(
        `Invalid transition: ${currentStatus} → ${newStatus} for ${destination}.${process}`
      );
    }

    // Update in travel-plan.json (process object)
    const dest = this.plan.destinations[destination];
    if (dest && dest[process]) {
      const processObj = dest[process] as Record<string, unknown>;
      processObj['status'] = newStatus;
      processObj['updated_at'] = this.timestamp;
    }

    // Update in state.json
    this.ensureEventLogDestination(destination);
    const destLog = this.eventLog.destinations[destination];
    if (!destLog.processes[process]) {
      destLog.processes[process] = { state: newStatus, events: [] };
    }
    destLog.processes[process].state = newStatus;

    // Emit event
    this.emitEvent({
      event: 'status_changed',
      destination,
      process,
      from: currentStatus ?? undefined,
      to: newStatus,
    });
  }

  /**
   * Get current process status.
   */
  getProcessStatus(destination: string, process: ProcessId): ProcessStatus | null {
    const dest = this.plan.destinations[destination];
    if (!dest || !dest[process]) return null;
    const processObj = dest[process] as Record<string, unknown>;
    return (processObj['status'] as ProcessStatus) || null;
  }

  /**
   * Check if transition is valid.
   */
  isValidTransition(from: ProcessStatus, to: ProcessStatus): boolean {
    const allowed = STATUS_TRANSITIONS[from];
    return allowed?.includes(to) ?? false;
  }

  // ============================================================================
  // Event Logging
  // ============================================================================

  /**
   * Emit an event to the audit log.
   */
  emitEvent(event: Omit<TravelEvent, 'at'>): void {
    const fullEvent: TravelEvent = {
      ...event,
      at: this.timestamp,
    };

    // Add to global event log
    this.eventLog.event_log.push(fullEvent);

    // Add to destination-specific log if applicable
    if (event.destination && event.process) {
      this.ensureEventLogDestination(event.destination);
      const destLog = this.eventLog.destinations[event.destination];
      if (!destLog.processes[event.process]) {
        destLog.processes[event.process] = { state: 'pending', events: [] };
      }
      destLog.processes[event.process].events.push(fullEvent);
    }
  }

  /**
   * Get event log.
   */
  getEventLog(): TravelEvent[] {
    return this.eventLog.event_log;
  }

  // ============================================================================
  // Date Anchor Management
  // ============================================================================

  /**
   * Set or update the date anchor (P1).
   * Triggers cascade to invalidate all date-dependent processes.
   * 
   * @param startDate - Departure date (ISO-8601: YYYY-MM-DD)
   * @param endDate - Return date (ISO-8601: YYYY-MM-DD)
   * @param reason - Optional reason for the change (e.g., "Agent offered Feb 13")
   */
  setDateAnchor(startDate: string, endDate: string, reason?: string): void {
    const dest = this.getActiveDestination();
    const destObj = this.plan.destinations[dest];
    
    if (!destObj) {
      throw new Error(`Active destination not found: ${dest}`);
    }

    // Get current dates for comparison
    const p1 = destObj.process_1_date_anchor as Record<string, unknown> | undefined;
    const currentDates = p1?.confirmed_dates as { start: string; end: string } | undefined;
    
    const oldStart = currentDates?.start;
    const oldEnd = currentDates?.end;
    
    // Calculate days
    const start = new Date(startDate);
    const end = new Date(endDate);
    const days = Math.ceil((end.getTime() - start.getTime()) / (1000 * 60 * 60 * 24)) + 1;

    // Update the date anchor
    if (!destObj.process_1_date_anchor) {
      (destObj as Record<string, unknown>).process_1_date_anchor = {};
    }
    const dateAnchor = destObj.process_1_date_anchor as Record<string, unknown>;
    dateAnchor.confirmed_dates = { start: startDate, end: endDate };
    dateAnchor.days = days;
    dateAnchor.updated_at = this.timestamp;

    // Use setProcessStatus for consistent state tracking
    this.setProcessStatus(dest, 'process_1_date_anchor', 'confirmed');

    // Emit event
    this.emitEvent({
      event: 'date_anchor_changed',
      destination: dest,
      process: 'process_1_date_anchor',
      data: { 
        from_dates: oldStart ? `${oldStart} to ${oldEnd}` : null,
        to_dates: `${startDate} to ${endDate}`,
        start: startDate, 
        end: endDate, 
        days,
        reason: reason || 'User updated dates'
      },
    });

    // Mark global dirty and trigger cascade
    this.markGlobalDirty('process_1_date_anchor');
    
    // Mark all date-dependent processes as dirty
    const dateDependentProcesses: ProcessId[] = [
      'process_3_transportation',
      'process_3_4_packages',
      'process_4_accommodation',
      'process_5_daily_itinerary',
    ];
    
    for (const process of dateDependentProcesses) {
      this.markDirty(dest, process);
    }
  }

  /**
   * Get current date anchor.
   */
  getDateAnchor(): { start: string; end: string; days: number } | null {
    const dest = this.getActiveDestination();
    const destObj = this.plan.destinations[dest];
    if (!destObj) return null;
    
    const p1 = destObj.process_1_date_anchor as Record<string, unknown> | undefined;
    const dates = p1?.confirmed_dates as { start: string; end: string } | undefined;
    const days = p1?.days as number | undefined;
    
    if (!dates) return null;
    return { start: dates.start, end: dates.end, days: days || 0 };
  }

  // ============================================================================
  // Offer Management (delegated to OfferManager)
  // ============================================================================

  /**
   * Update availability for a specific offer/date combination.
   * Use this when agent provides new info (e.g., "Feb 13 is now available").
   *
   * @param offerId - The offer ID (e.g., "besttour_TYO05MM260211AM")
   * @param date - The date to update (ISO-8601: YYYY-MM-DD)
   * @param availability - New availability status
   * @param price - Optional new price
   * @param seatsRemaining - Optional seats remaining
   * @param source - Source of info (e.g., "agent", "scrape", "user")
   */
  updateOfferAvailability(
    offerId: string,
    date: string,
    availability: 'available' | 'sold_out' | 'limited',
    price?: number,
    seatsRemaining?: number,
    source: string = 'user'
  ): void {
    const dest = this.getActiveDestination();
    const destObj = this.plan.destinations[dest];
    
    if (!destObj) {
      throw new Error(`Active destination not found: ${dest}`);
    }

    // Find the offer in packages
    const packages = destObj.process_3_4_packages as Record<string, unknown> | undefined;
    const results = packages?.results as Record<string, unknown> | undefined;
    const offers = results?.offers as Array<Record<string, unknown>> | undefined;

    if (!offers) {
      throw new Error(`No offers found in ${dest}.process_3_4_packages.results`);
    }

    const offer = offers.find(o => o.id === offerId);
    if (!offer) {
      throw new Error(`Offer not found: ${offerId}`);
    }

    // Update date_pricing
    let datePricing = offer.date_pricing as Record<string, Record<string, unknown>> | undefined;
    if (!datePricing) {
      datePricing = {};
      offer.date_pricing = datePricing;
    }

    const previousEntry = datePricing[date];
    const previousAvailability = previousEntry?.availability;

    datePricing[date] = {
      ...previousEntry,
      availability,
      ...(price !== undefined && { price }),
      ...(seatsRemaining !== undefined && { seats_remaining: seatsRemaining }),
      note: `Updated by ${source} at ${this.timestamp}`,
    };

    // Emit event
    this.emitEvent({
      event: 'offer_availability_updated',
      destination: dest,
      process: 'process_3_4_packages',
      data: {
        offer_id: offerId,
        date,
        from: previousAvailability,
        to: availability,
        price,
        seats_remaining: seatsRemaining,
        source,
      },
    });
  }

  /**
   * Select an offer for booking.
   * This marks the package as selected and can trigger cascade populate
   * to fill P3 (transportation) and P4 (accommodation) from the offer.
   * 
   * @param offerId - The offer ID to select
   * @param date - The specific date to book
   * @param populateCascade - If true, populate P3/P4 from offer details
   */
  selectOffer(offerId: string, date: string, populateCascade: boolean = true): void {
    const dest = this.getActiveDestination();
    const destObj = this.plan.destinations[dest];
    
    if (!destObj) {
      throw new Error(`Active destination not found: ${dest}`);
    }

    const packages = destObj.process_3_4_packages as Record<string, unknown> | undefined;
    const results = packages?.results as Record<string, unknown> | undefined;
    const offers = results?.offers as Array<Record<string, unknown>> | undefined;

    if (!offers) {
      throw new Error(`No offers found in ${dest}.process_3_4_packages.results`);
    }

    const offer = offers.find(o => o.id === offerId);
    if (!offer) {
      throw new Error(`Offer not found: ${offerId}`);
    }

    // Mark as chosen
    if (!packages) {
      throw new Error('Packages process not found');
    }
    packages.selected_offer_id = offerId;
    packages.chosen_offer = {
      id: offerId,
      selected_date: date,
      selected_at: this.timestamp,
    };
    if (!packages.results || typeof packages.results !== 'object') {
      packages.results = {};
    }
    (packages.results as Record<string, unknown>).chosen_offer = offer;

    // Update process status
    this.setProcessStatus(dest, 'process_3_4_packages', 'selected');

    // Emit event
    this.emitEvent({
      event: 'offer_selected',
      destination: dest,
      process: 'process_3_4_packages',
      data: {
        offer_id: offerId,
        date,
        offer_name: offer.name,
        hotel: (offer.hotel as Record<string, unknown>)?.name,
        price_total: (offer.date_pricing as Record<string, Record<string, unknown>>)?.[date]?.price,
      },
    });

    // Populate cascade: fill P3 and P4 from the selected offer
    if (populateCascade) {
      this.populateFromOffer(dest, offer, date);
    }
  }

  /**
   * Populate P3 (transportation) and P4 (accommodation) from selected offer.
   * @internal
   */
  private populateFromOffer(
    destination: string,
    offer: Record<string, unknown>,
    date: string
  ): void {
    const destObj = this.plan.destinations[destination];
    if (!destObj) return;

    // Populate P3 (transportation) from offer flight info
    const flight = offer.flight as Record<string, unknown> | undefined;
    if (flight) {
      const p3 = destObj.process_3_transportation as Record<string, unknown> | undefined;
      if (p3) {
        p3.populated_from = `package:${offer.id}`;
        p3.flight = {
          ...flight,
          booked_date: date,
          populated_at: this.timestamp,
        };
        this.setProcessStatus(destination, 'process_3_transportation', 'populated');
        this.clearDirty(destination, 'process_3_transportation');
      }
    }

    // Populate P4 (accommodation) from offer hotel info
    const hotel = offer.hotel as Record<string, unknown> | undefined;
    if (hotel) {
      const p4 = destObj.process_4_accommodation as Record<string, unknown> | undefined;
      if (p4) {
        p4.populated_from = `package:${offer.id}`;
        p4.hotel = {
          ...hotel,
          check_in: date,
          populated_at: this.timestamp,
        };
        this.setProcessStatus(destination, 'process_4_accommodation', 'populated');
        this.clearDirty(destination, 'process_4_accommodation');
      }
    }

    this.emitEvent({
      event: 'cascade_populated',
      destination,
      data: {
        source: `package:${offer.id}`,
        populated: ['process_3_transportation', 'process_4_accommodation'],
      },
    });
  }

  /**
   * Import normalized package offers (P3+4) into the current destination.
   * This is typically fed by scrapers and should be used instead of direct JSON edits.
   */
  importPackageOffers(
    destination: string,
    sourceId: string,
    offers: Array<Record<string, unknown>>,
    note?: string,
    warnings?: string[]
  ): void {
    const destObj = this.plan.destinations[destination];
    if (!destObj) {
      throw new Error(`Destination not found: ${destination}`);
    }

    if (!destObj.process_3_4_packages) {
      (destObj as Record<string, unknown>).process_3_4_packages = {};
    }

    const p34 = destObj.process_3_4_packages as Record<string, unknown>;
    if (!p34.results || typeof p34.results !== 'object') {
      p34.results = {};
    }
    const results = p34.results as Record<string, unknown>;

    results.offers = offers;
    const provenance = (results.provenance as Array<Record<string, unknown>> | undefined) ?? [];
    provenance.push({
      source_id: sourceId,
      scraped_at: this.timestamp,
      offers_found: offers.length,
      ...(note ? { note } : {}),
    });
    results.provenance = provenance;

    if (warnings && warnings.length > 0) {
      const existing = (results.warnings as string[] | undefined) ?? [];
      results.warnings = [...existing, ...warnings];
    }

    const currentStatus = this.getProcessStatus(destination, 'process_3_4_packages');
    if (!currentStatus || currentStatus === 'pending' || currentStatus === 'researching') {
      this.setProcessStatus(destination, 'process_3_4_packages', 'researched');
    } else {
      // still bump timestamp on the process node for visibility
      p34.updated_at = this.timestamp;
    }

    this.emitEvent({
      event: 'package_offers_imported',
      destination,
      process: 'process_3_4_packages',
      data: { source_id: sourceId, offers_found: offers.length, note },
    });
  }

  // ============================================================================
  // Transportation (Ground Transfers)
  // ============================================================================

  /**
   * Set airport transfer segment for arrival/departure.
   */
  setAirportTransferSegment(
    destination: string,
    direction: 'arrival' | 'departure',
    segment: TransportSegment
  ): void {
    const p3 = this.ensureTransportationProcess(destination);

    if (!p3.airport_transfers || typeof p3.airport_transfers !== 'object') {
      p3.airport_transfers = {};
    }

    (p3.airport_transfers as Record<string, unknown>)[direction] = segment as unknown as Record<string, unknown>;
    this.touchTransportation(destination);

    this.emitEvent({
      event: 'airport_transfer_updated',
      destination,
      process: 'process_3_transportation',
      data: {
        direction,
        status: segment.status,
        selected_id: segment.selected?.id ?? null,
        candidates_count: segment.candidates?.length ?? 0,
      },
    });
  }

  /**
   * Add a candidate option to the airport transfer segment.
   */
  addAirportTransferCandidate(
    destination: string,
    direction: 'arrival' | 'departure',
    option: TransportOption
  ): void {
    const p3 = this.ensureTransportationProcess(destination);
    if (!p3.airport_transfers || typeof p3.airport_transfers !== 'object') {
      p3.airport_transfers = {};
    }

    const transfers = p3.airport_transfers as Record<string, unknown>;
    const existing = (transfers[direction] as Record<string, unknown> | undefined) ?? {
      status: 'planned',
      selected: null,
      candidates: [],
    };

    const candidates = (existing.candidates as TransportOption[] | undefined) ?? [];
    if (!candidates.some(c => c.id === option.id)) {
      candidates.push(option);
    }
    existing.candidates = candidates;
    transfers[direction] = existing;

    this.touchTransportation(destination);
    this.emitEvent({
      event: 'airport_transfer_candidate_added',
      destination,
      process: 'process_3_transportation',
      data: { direction, option_id: option.id, title: option.title },
    });
  }

  /**
   * Select a candidate option as the chosen airport transfer.
   */
  selectAirportTransferOption(
    destination: string,
    direction: 'arrival' | 'departure',
    optionId: string
  ): void {
    const p3 = this.ensureTransportationProcess(destination);
    if (!p3.airport_transfers || typeof p3.airport_transfers !== 'object') {
      throw new Error(`No airport transfers set for ${destination}`);
    }

    const transfers = p3.airport_transfers as Record<string, unknown>;
    const segment = transfers[direction] as Record<string, unknown> | undefined;
    if (!segment) {
      throw new Error(`No ${direction} airport transfer segment found`);
    }

    const candidates = (segment.candidates as TransportOption[] | undefined) ?? [];
    const selected = candidates.find(c => c.id === optionId) as TransportOption | undefined;
    if (!selected) {
      throw new Error(`Airport transfer option not found: ${optionId}`);
    }

    segment.selected = selected;
    transfers[direction] = segment;

    this.touchTransportation(destination);
    this.emitEvent({
      event: 'airport_transfer_selected',
      destination,
      process: 'process_3_transportation',
      data: { direction, option_id: optionId, title: selected.title },
    });
  }

  // ============================================================================
  // Itinerary Management
  // ============================================================================

  /**
   * Scaffold day skeletons for P5 itinerary.
   * Creates empty day structures based on travel dates.
   *
   * @param destination - Destination slug
   * @param days - Array of day skeleton objects
   * @param force - If true, reset P5 to pending first to allow re-scaffolding
   */
  scaffoldItinerary(
    destination: string,
    days: Array<Record<string, unknown>>,
    force: boolean = false
  ): void {
    const destObj = this.plan.destinations[destination];
    if (!destObj) {
      throw new Error(`Destination not found: ${destination}`);
    }

    if (!destObj.process_5_daily_itinerary) {
      (destObj as Record<string, unknown>).process_5_daily_itinerary = {};
    }

    const p5 = destObj.process_5_daily_itinerary as Record<string, unknown>;
    const currentStatus = this.getProcessStatus(destination, 'process_5_daily_itinerary');

    // If force and status is beyond researching, reset to pending first
    if (force && currentStatus && !['pending', 'researching'].includes(currentStatus)) {
      this.forceSetProcessStatus(destination, 'process_5_daily_itinerary', 'pending', {
        reason: 'force re-scaffold',
        from: currentStatus,
      });
    }

    p5.days = days;
    p5.updated_at = this.timestamp;
    p5.scaffolded_at = this.timestamp;

    // Set status to researching (skeleton created, content still pending)
    this.setProcessStatus(destination, 'process_5_daily_itinerary', 'researching');
    this.clearDirty(destination, 'process_5_daily_itinerary');

    this.emitEvent({
      event: 'itinerary_scaffolded',
      destination,
      process: 'process_5_daily_itinerary',
      data: {
        days_count: days.length,
        day_types: days.map(d => d.day_type),
      },
    });
  }

  // ============================================================================
  // Activity CRUD (P5 Itinerary)
  // ============================================================================

  /**
   * Add an activity to a day session.
   * @param destination - Destination slug
   * @param dayNumber - 1-indexed day number
   * @param session - 'morning' | 'afternoon' | 'evening'
   * @param activity - Activity object (id will be generated if not provided)
   * @returns The generated activity ID
   */
  addActivity(
    destination: string,
    dayNumber: number,
    session: 'morning' | 'afternoon' | 'evening',
    activity: {
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
  ): string {
    const day = this.getDay(destination, dayNumber);
    if (!day) {
      throw new Error(`Day ${dayNumber} not found in ${destination}`);
    }

    const sessionObj = day[session] as { activities: Array<Record<string, unknown>> };
    if (!sessionObj || !Array.isArray(sessionObj.activities)) {
      throw new Error(`Session ${session} not found in Day ${dayNumber}`);
    }

    // Generate ID
    const id = `activity_${Date.now().toString(36)}_${Math.random().toString(36).substring(2, 6)}`;

    const fullActivity = {
      id,
      title: activity.title,
      area: activity.area || '',
      nearest_station: activity.nearest_station || null,
      duration_min: activity.duration_min || null,
      booking_required: activity.booking_required || false,
      booking_url: activity.booking_url || null,
      cost_estimate: activity.cost_estimate || null,
      tags: activity.tags || [],
      notes: activity.notes || null,
      priority: activity.priority || 'want',
    };

    sessionObj.activities.push(fullActivity);
    this.touchItinerary(destination);

    this.emitEvent({
      event: 'activity_added',
      destination,
      process: 'process_5_daily_itinerary',
      data: { day_number: dayNumber, session, activity_id: id, title: activity.title },
    });

    return id;
  }

  /**
   * Update an existing activity.
   */
  updateActivity(
    destination: string,
    dayNumber: number,
    session: 'morning' | 'afternoon' | 'evening',
    activityId: string,
    updates: Partial<{
      title: string;
      area: string;
      nearest_station: string;
      duration_min: number;
      booking_required: boolean;
      booking_url: string;
      start_time: string;
      end_time: string;
      is_fixed_time: boolean;
      cost_estimate: number;
      tags: string[];
      notes: string;
      priority: 'must' | 'want' | 'optional';
    }>
  ): void {
    const day = this.getDay(destination, dayNumber);
    if (!day) {
      throw new Error(`Day ${dayNumber} not found in ${destination}`);
    }

    const sessionObj = day[session] as { activities: Array<string | Record<string, unknown>> };
    if (!sessionObj?.activities) {
      throw new Error(`Session ${session} not found in Day ${dayNumber}`);
    }

    const idx = this.findActivityIndex(sessionObj.activities, activityId);
    if (idx === -1) {
      throw new Error(`Activity ${activityId} not found in Day ${dayNumber} ${session}`);
    }

    const current = sessionObj.activities[idx];
    const activityObj = typeof current === 'string'
      ? this.upgradeStringActivity(current, { booking_required: false })
      : current;
    sessionObj.activities[idx] = activityObj;
    Object.assign(activityObj, updates);
    this.touchItinerary(destination);

    this.emitEvent({
      event: 'activity_updated',
      destination,
      process: 'process_5_daily_itinerary',
      data: { day_number: dayNumber, session, activity_id: activityObj.id, updates: Object.keys(updates) },
    });
  }

  /**
   * Set time fields for an activity (start/end/fixed-time).
   * Activity can be found by ID or title; legacy string activities are upgraded.
   */
  setActivityTime(
    destination: string,
    dayNumber: number,
    session: 'morning' | 'afternoon' | 'evening',
    activityIdOrTitle: string,
    opts: { start_time?: string; end_time?: string; is_fixed_time?: boolean }
  ): void {
    const day = this.getDay(destination, dayNumber);
    if (!day) {
      throw new Error(`Day ${dayNumber} not found in ${destination}`);
    }

    const sessionObj = day[session] as { activities: Array<string | Record<string, unknown>> };
    if (!sessionObj?.activities) {
      throw new Error(`Session ${session} not found in Day ${dayNumber}`);
    }

    const idx = this.findActivityIndex(sessionObj.activities, activityIdOrTitle);
    if (idx === -1) {
      throw new Error(`Activity not found: "${activityIdOrTitle}" in Day ${dayNumber} ${session}`);
    }

    const current = sessionObj.activities[idx];
    const activityObj = typeof current === 'string'
      ? this.upgradeStringActivity(current, { booking_required: false })
      : current;
    sessionObj.activities[idx] = activityObj;

    const previous = {
      start_time: activityObj.start_time as string | undefined,
      end_time: activityObj.end_time as string | undefined,
      is_fixed_time: activityObj.is_fixed_time as boolean | undefined,
    };

    if (opts.start_time !== undefined) activityObj.start_time = opts.start_time;
    if (opts.end_time !== undefined) activityObj.end_time = opts.end_time;
    if (opts.is_fixed_time !== undefined) activityObj.is_fixed_time = opts.is_fixed_time;

    this.touchItinerary(destination);
    this.emitEvent({
      event: 'activity_time_updated',
      destination,
      process: 'process_5_daily_itinerary',
      data: {
        day_number: dayNumber,
        session,
        activity_id: activityObj.id,
        title: activityObj.title,
        from: previous,
        to: {
          start_time: activityObj.start_time,
          end_time: activityObj.end_time,
          is_fixed_time: activityObj.is_fixed_time,
        },
      },
    });
  }

  /**
   * Set optional time range boundary for a session.
   */
  setSessionTimeRange(
    destination: string,
    dayNumber: number,
    session: 'morning' | 'afternoon' | 'evening',
    start: string,
    end: string
  ): void {
    const day = this.getDay(destination, dayNumber);
    if (!day) {
      throw new Error(`Day ${dayNumber} not found in ${destination}`);
    }

    const sessionObj = day[session] as Record<string, unknown> | undefined;
    if (!sessionObj) {
      throw new Error(`Session ${session} not found in Day ${dayNumber}`);
    }

    sessionObj.time_range = { start, end };
    this.touchItinerary(destination);

    this.emitEvent({
      event: 'session_time_range_updated',
      destination,
      process: 'process_5_daily_itinerary',
      data: { day_number: dayNumber, session, start, end },
    });
  }

  /**
   * Remove an activity.
   */
  removeActivity(
    destination: string,
    dayNumber: number,
    session: 'morning' | 'afternoon' | 'evening',
    activityId: string
  ): void {
    const day = this.getDay(destination, dayNumber);
    if (!day) {
      throw new Error(`Day ${dayNumber} not found in ${destination}`);
    }

    const sessionObj = day[session] as { activities: Array<string | Record<string, unknown>> };
    if (!sessionObj?.activities) {
      throw new Error(`Session ${session} not found in Day ${dayNumber}`);
    }

    const idx = this.findActivityIndex(sessionObj.activities, activityId);
    if (idx === -1) {
      throw new Error(`Activity ${activityId} not found in Day ${dayNumber} ${session}`);
    }

    const removed = sessionObj.activities.splice(idx, 1)[0];
    this.touchItinerary(destination);

    this.emitEvent({
      event: 'activity_removed',
      destination,
      process: 'process_5_daily_itinerary',
      data: {
        day_number: dayNumber,
        session,
        activity_id: typeof removed === 'string' ? null : removed.id,
        title: typeof removed === 'string' ? removed : (removed.title as string | undefined),
      },
    });
  }

  /**
   * Set booking status for an activity.
   * Use this to track booking progress for activities that require advance booking.
   *
   * Handles both legacy string activities and structured Activity objects:
   * - String activities are upgraded to Activity objects with generated IDs
   * - Object activities are updated in place
   *
   * @param destination - Destination slug
   * @param dayNumber - 1-indexed day number
   * @param session - 'morning' | 'afternoon' | 'evening'
   * @param activityIdOrTitle - Activity ID or title (case-insensitive, substring match for strings)
   * @param status - Booking status: 'not_required' | 'pending' | 'booked' | 'waitlist'
   * @param ref - Optional booking reference/confirmation number
   * @param bookBy - Optional deadline to book (ISO date: YYYY-MM-DD)
   */
  setActivityBookingStatus(
    destination: string,
    dayNumber: number,
    session: 'morning' | 'afternoon' | 'evening',
    activityIdOrTitle: string,
    status: 'not_required' | 'pending' | 'booked' | 'waitlist',
    ref?: string,
    bookBy?: string
  ): void {
    const day = this.getDay(destination, dayNumber);
    if (!day) {
      throw new Error(`Day ${dayNumber} not found in ${destination}`);
    }

    const sessionObj = day[session] as { activities: Array<string | Record<string, unknown>> };
    if (!sessionObj?.activities) {
      throw new Error(`Session ${session} not found in Day ${dayNumber}`);
    }

    const activityIdx = this.findActivityIndex(sessionObj.activities, activityIdOrTitle);
    if (activityIdx === -1) {
      throw new Error(
        `Activity not found: "${activityIdOrTitle}" in Day ${dayNumber} ${session}`
      );
    }

    const activity = sessionObj.activities[activityIdx];
    const wasUpgraded = typeof activity === 'string';
    const activityObj = wasUpgraded
      ? this.upgradeStringActivity(activity, { booking_required: true })
      : activity;
    if (wasUpgraded) {
      sessionObj.activities[activityIdx] = activityObj;
    }

    const previousStatus = activityObj.booking_status as string | undefined;
    activityObj.booking_status = status;

    if (ref !== undefined) {
      activityObj.booking_ref = ref;
    }

    if (bookBy !== undefined) {
      activityObj.book_by = bookBy;
    }

    // If marking as booked, also set booking_required to true for consistency
    if (status === 'booked' || status === 'pending' || status === 'waitlist') {
      activityObj.booking_required = true;
    }

    this.touchItinerary(destination);

    this.emitEvent({
      event: 'activity_booking_updated',
      destination,
      process: 'process_5_daily_itinerary',
      data: {
        day_number: dayNumber,
        session,
        activity_id: activityObj.id,
        title: activityObj.title,
        from_status: previousStatus,
        to_status: status,
        booking_ref: ref,
        book_by: bookBy,
        upgraded_from_string: wasUpgraded,
      },
    });
  }

  /**
   * Find an activity by ID or title across all days/sessions.
   * Handles both string activities and Activity objects.
   * Returns { dayNumber, session, activity, isString } or null if not found.
   */
  findActivity(
    destination: string,
    idOrTitle: string
  ): { dayNumber: number; session: 'morning' | 'afternoon' | 'evening'; activity: string | Record<string, unknown>; isString: boolean } | null {
    const destObj = this.plan.destinations[destination];
    if (!destObj) return null;

    const p5 = destObj.process_5_daily_itinerary as Record<string, unknown> | undefined;
    const days = p5?.days as Array<Record<string, unknown>> | undefined;
    if (!days) return null;

    const sessions: Array<'morning' | 'afternoon' | 'evening'> = ['morning', 'afternoon', 'evening'];
    const searchLower = idOrTitle.toLowerCase();

    for (const day of days) {
      const dayNumber = day.day_number as number;
      for (const session of sessions) {
        const sessionObj = day[session] as { activities: Array<string | Record<string, unknown>> } | undefined;
        if (!sessionObj?.activities) continue;

        for (const a of sessionObj.activities) {
          if (typeof a === 'string') {
            if (a.toLowerCase().includes(searchLower)) {
              return { dayNumber, session, activity: a, isString: true };
            }
          } else {
            const id = a.id as string | undefined;
            const title = a.title as string | undefined;
            if (id === idOrTitle ||
                (title && title.toLowerCase().includes(searchLower))) {
              return { dayNumber, session, activity: a, isString: false };
            }
          }
        }
      }
    }

    return null;
  }

  /**
   * Get a specific day from itinerary.
   */
  private getDay(destination: string, dayNumber: number): Record<string, unknown> | null {
    const destObj = this.plan.destinations[destination];
    if (!destObj) return null;

    const p5 = destObj.process_5_daily_itinerary as Record<string, unknown> | undefined;
    const days = p5?.days as Array<Record<string, unknown>> | undefined;
    if (!days) return null;

    return days.find(d => d.day_number === dayNumber) || null;
  }

  /**
   * Set a day's theme field.
   */
  setDayTheme(destination: string, dayNumber: number, theme: string | null): void {
    const day = this.getDay(destination, dayNumber);
    if (!day) {
      throw new Error(`Day ${dayNumber} not found in ${destination}`);
    }

    day.theme = theme;
    this.touchItinerary(destination);

    this.emitEvent({
      event: 'itinerary_day_theme_set',
      destination,
      process: 'process_5_daily_itinerary',
      data: { day_number: dayNumber, theme },
    });
  }

  /**
   * Set weather forecast for a specific day.
   */
  setDayWeather(destination: string, dayNumber: number, weather: import('./types').DayWeather): void {
    const day = this.getDay(destination, dayNumber);
    if (!day) {
      throw new Error(`Day ${dayNumber} not found in ${destination}`);
    }

    (day as Record<string, unknown>).weather = weather;
    this.touchItinerary(destination);

    this.emitEvent({
      event: 'weather_updated',
      destination,
      process: 'process_5_daily_itinerary',
      data: { day_number: dayNumber, source_id: weather.source_id },
    });
  }

  /**
   * Set a session's focus field.
   */
  setSessionFocus(
    destination: string,
    dayNumber: number,
    session: 'morning' | 'afternoon' | 'evening',
    focus: string | null
  ): void {
    const day = this.getDay(destination, dayNumber);
    if (!day) {
      throw new Error(`Day ${dayNumber} not found in ${destination}`);
    }

    const sessionObj = day[session] as Record<string, unknown> | undefined;
    if (!sessionObj) {
      throw new Error(`Session ${session} not found in Day ${dayNumber}`);
    }

    sessionObj.focus = focus;
    this.touchItinerary(destination);

    this.emitEvent({
      event: 'itinerary_session_focus_set',
      destination,
      process: 'process_5_daily_itinerary',
      data: { day_number: dayNumber, session, focus },
    });
  }

  /**
   * Touch itinerary timestamp.
   */
  private touchItinerary(destination: string): void {
    const destObj = this.plan.destinations[destination];
    if (!destObj) return;

    const p5 = destObj.process_5_daily_itinerary as Record<string, unknown> | undefined;
    if (p5) {
      p5.updated_at = this.timestamp;
    }
  }

  /**
   * Touch transportation timestamp.
   */
  private touchTransportation(destination: string): void {
    const destObj = this.plan.destinations[destination];
    if (!destObj) return;

    const p3 = destObj.process_3_transportation as Record<string, unknown> | undefined;
    if (p3) {
      p3.updated_at = this.timestamp;
    }
  }

  private ensureTransportationProcess(destination: string): Record<string, unknown> {
    const destObj = this.plan.destinations[destination];
    if (!destObj) {
      throw new Error(`Destination not found: ${destination}`);
    }

    if (!destObj.process_3_transportation) {
      (destObj as Record<string, unknown>).process_3_transportation = {
        status: 'pending',
        updated_at: this.timestamp,
      };
    }

    const p3 = destObj.process_3_transportation as Record<string, unknown>;
    if (typeof p3.status !== 'string') {
      p3.status = 'pending';
    }
    return p3;
  }

  private upgradeStringActivity(
    title: string,
    overrides?: Partial<Record<string, unknown>>
  ): Record<string, unknown> {
    const id = `activity_${Date.now().toString(36)}_${Math.random().toString(36).substring(2, 6)}`;
    return {
      id,
      title,
      area: '',
      nearest_station: null,
      duration_min: null,
      booking_required: false,
      booking_url: null,
      booking_status: undefined,
      booking_ref: undefined,
      book_by: undefined,
      cost_estimate: null,
      tags: [],
      notes: null,
      priority: 'want',
      ...(overrides || {}),
    };
  }

  /**
   * Find activity index by ID (exact match) or title (case-insensitive substring).
   * Returns -1 if not found.
   *
   * @internal Shared helper to reduce duplication in activity methods.
   */
  private findActivityIndex(
    activities: Array<string | Record<string, unknown>>,
    idOrTitle: string
  ): number {
    // First try exact ID match
    const idx = activities.findIndex(
      a => typeof a !== 'string' && a.id === idOrTitle
    );
    if (idx !== -1) return idx;

    // Fall back to title substring (case-insensitive)
    const searchLower = idOrTitle.toLowerCase();
    return activities.findIndex(a => {
      if (typeof a === 'string') {
        return a.toLowerCase().includes(searchLower);
      }
      const title = a.title as string | undefined;
      return Boolean(title && title.toLowerCase().includes(searchLower));
    });
  }

  /**
   * Force-set a process status without transition validation.
   * Use sparingly for recovery/override paths (e.g. re-scaffolding).
   */
  private forceSetProcessStatus(
    destination: string,
    process: ProcessId,
    newStatus: ProcessStatus,
    data?: Record<string, unknown>
  ): void {
    const currentStatus = this.getProcessStatus(destination, process);

    // Update in travel-plan.json (process object)
    const dest = this.plan.destinations[destination];
    if (dest && dest[process]) {
      const processObj = dest[process] as Record<string, unknown>;
      processObj['status'] = newStatus;
      processObj['updated_at'] = this.timestamp;
    }

    // Update in state.json
    this.ensureEventLogDestination(destination);
    const destLog = this.eventLog.destinations[destination];
    if (!destLog.processes[process]) {
      destLog.processes[process] = { state: newStatus, events: [] };
    }
    destLog.processes[process].state = newStatus;

    this.emitEvent({
      event: 'status_forced',
      destination,
      process,
      from: currentStatus ?? undefined,
      to: newStatus,
      data,
    });
  }

  // ============================================================================
  // Active Destination
  // ============================================================================

  /**
   * Get active destination slug.
   */
  getActiveDestination(): string {
    return this.plan.active_destination;
  }

  /**
   * Set active destination and track change for cascade.
   */
  setActiveDestination(destination: string): void {
    const previous = this.plan.active_destination;
    if (previous !== destination) {
      this.plan.cascade_state.global.active_destination_last = previous;
      this.plan.active_destination = destination;
      this.eventLog.active_destination = destination;
      this.emitEvent({
        event: 'active_destination_changed',
        data: { from: previous, to: destination },
      });
    }
  }

  /**
   * Set current focus (for UI/skill coordination).
   */
  setFocus(destination: string, process: ProcessId): void {
    const previous = this.eventLog.current_focus;
    const newFocus = `${destination}.${process}`;
    this.eventLog.current_focus = newFocus;
    this.emitEvent({
      event: 'focus_changed',
      destination,
      process,
      data: { from: previous, to: newFocus },
    });
  }

  /**
   * Set session-level next actions (global; not per-destination).
   */
  setNextActions(actions: string[]): void {
    const previous = this.eventLog.next_actions || [];
    this.eventLog.next_actions = actions;
    this.emitEvent({
      event: 'next_actions_updated',
      data: { from: previous, to: actions },
    });
  }

  /**
   * Get next actions list.
   */
  getNextActions(): string[] {
    return this.eventLog.next_actions || [];
  }

  // ============================================================================
  // File I/O
  // ============================================================================

  /**
   * Load travel plan from file with Zod validation.
   */
  loadPlan(path?: string): TravelPlanMinimal {
    const filePath = path || this.planPath;
    if (!existsSync(filePath)) {
      throw new Error(`Travel plan not found: ${filePath}`);
    }
    const content = readFileSync(filePath, 'utf-8');
    const parsed = JSON.parse(content);

    // Validate with Zod
    return validateTravelPlan(parsed) as TravelPlanMinimal;
  }

  /**
   * Load event log from file with Zod validation.
   */
  loadEventLog(path?: string): EventLogState {
    const filePath = path || this.statePath;
    if (!existsSync(filePath)) {
      // Return minimal state if file doesn't exist
      return {
        session: new Date().toISOString().split('T')[0],
        project: DEFAULTS.project,
        version: '3.0',
        active_destination: this.plan?.active_destination || '',
        current_focus: '',
        event_log: [],
        global_processes: {},
        destinations: {},
      };
    }
    const content = readFileSync(filePath, 'utf-8');
    const parsed = JSON.parse(content);

    // Validate with Zod
    return validateEventLogState(parsed) as EventLogState;
  }

  /**
   * Save plan + state to DB, then sync derived tables.
   * Validates before saving to prevent corrupt data.
   * Note: Does NOT update last_cascade_run - that is cascade-runner-owned.
   */
  async save(): Promise<void> {
    // Validate before saving (catch bugs before writing corrupt data)
    validateTravelPlan(this.plan);
    validateEventLogState(this.eventLog);

    // Skip all writes in test mode
    if (this.skipSave) return;

    const planJson = JSON.stringify(this.plan, null, 2);
    const stateJson = JSON.stringify(this.eventLog, null, 2);
    const schemaVersion = this.plan.schema_version || 'unknown';

    // 1. Write to DB (blocking)
    try {
      const { writePlanToDb } = require('../services/turso-service');
      await writePlanToDb(this.planId, planJson, stateJson, schemaVersion);
    } catch (e: any) {
      throw new Error(`DB write failed — save aborted: ${e.message}`);
    }

    // 2. Fire-and-forget derived table sync (bookings + events)
    this.syncDerivedData();
  }

  /**
   * Fire-and-forget sync of derived data to Turso (bookings + events).
   * Primary plan data is already safe in DB; these are secondary tables.
   * @internal
   */
  private syncDerivedData(): void {
    try {
      const { syncBookingsFromPlan, syncEventsToDb } = require('../services/turso-service');
      syncBookingsFromPlan(this.planPath).catch((e: Error) => {
        console.warn(`  [turso] booking sync failed: ${e.message} — run 'npm run travel -- sync-bookings' to retry`);
      });
      syncEventsToDb(this.eventLog.event_log).catch((e: Error) => {
        console.warn(`  [turso] event sync failed: ${e.message}`);
      });
    } catch {
      // turso-service not available
    }
  }

  /**
   * Get current plan (for reading).
   */
  getPlan(): TravelPlanMinimal {
    return this.plan;
  }

  // ============================================================================
  // Helpers
  // ============================================================================

  private ensureDestinationState(destination: string): void {
    if (!this.plan.cascade_state.destinations[destination]) {
      this.plan.cascade_state.destinations[destination] = {};
    }
  }

  private ensureEventLogDestination(destination: string): void {
    if (!this.eventLog.destinations[destination]) {
      this.eventLog.destinations[destination] = {
        status: 'active',
        processes: {},
      };
    }
  }

  /**
   * Normalize known schema variants to the current contract.
   * This keeps skills/CLI resilient when older plan shapes exist on disk.
   */
  private normalizePlan(): void {
    for (const dest of Object.values(this.plan.destinations)) {
      const p34 = (dest as Record<string, unknown>)['process_3_4_packages'] as Record<string, unknown> | undefined;
      if (!p34 || typeof p34 !== 'object') continue;

      // Ensure results object exists.
      if (!p34['results'] || typeof p34['results'] !== 'object') {
        p34['results'] = {};
      }
      const results = p34['results'] as Record<string, unknown>;

      // Legacy: offers at process_3_4_packages.offers
      if (Array.isArray(p34['offers']) && !Array.isArray(results['offers'])) {
        results['offers'] = p34['offers'];
        delete p34['offers'];
      }

      // Legacy: chosen offer at process_3_4_packages.chosen_offer (full offer object).
      // Current: selection metadata stored at chosen_offer, full offer stored at results.chosen_offer.
      if (p34['chosen_offer'] && results['chosen_offer'] == null) {
        const maybeMeta = p34['chosen_offer'] as Record<string, unknown>;
        if (typeof maybeMeta?.id === 'string' && typeof maybeMeta?.selected_date === 'string') {
          // This is metadata; keep it as-is.
        } else {
          // Treat as legacy full-offer object.
          results['chosen_offer'] = p34['chosen_offer'];
        }
      }

      // Backfill selected_offer_id from results.chosen_offer where possible.
      if (p34['selected_offer_id'] == null && results['chosen_offer'] && typeof results['chosen_offer'] === 'object') {
        const id = (results['chosen_offer'] as Record<string, unknown>)['id'];
        if (typeof id === 'string') {
          p34['selected_offer_id'] = id;
        }
      }
    }
  }
}

// Singleton instance for convenience
let defaultInstance: StateManager | null = null;

export function getStateManager(): StateManager {
  if (!defaultInstance) {
    defaultInstance = new StateManager();
  }
  return defaultInstance;
}

export function resetStateManager(): void {
  defaultInstance = null;
}
