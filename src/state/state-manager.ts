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

import { readFileSync, writeFileSync, existsSync } from 'fs';
import {
  ProcessStatus,
  ProcessId,
  DirtyFlag,
  CascadeState,
  TravelEvent,
  EventLogState,
  TravelPlanMinimal,
  STATUS_TRANSITIONS,
} from './types';

// Default paths
const DEFAULT_PLAN_PATH = 'data/travel-plan.json';
const DEFAULT_STATE_PATH = 'data/state.json';

export class StateManager {
  private planPath: string;
  private statePath: string;
  private plan: TravelPlanMinimal;
  private eventLog: EventLogState;
  private timestamp: string;

  constructor(planPath?: string, statePath?: string) {
    this.planPath = planPath || DEFAULT_PLAN_PATH;
    this.statePath = statePath || DEFAULT_STATE_PATH;
    this.timestamp = this.freshTimestamp();
    this.plan = this.loadPlan();
    this.normalizePlan();
    this.eventLog = this.loadEventLog();
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
  // Offer Management
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
  setFocus(destination: string, process: string): void {
    this.eventLog.current_focus = `${destination}.${process}`;
  }

  // ============================================================================
  // File I/O
  // ============================================================================

  /**
   * Load travel plan from file.
   */
  loadPlan(path?: string): TravelPlanMinimal {
    const filePath = path || this.planPath;
    if (!existsSync(filePath)) {
      throw new Error(`Travel plan not found: ${filePath}`);
    }
    const content = readFileSync(filePath, 'utf-8');
    return JSON.parse(content) as TravelPlanMinimal;
  }

  /**
   * Load event log from file.
   */
  loadEventLog(path?: string): EventLogState {
    const filePath = path || this.statePath;
    if (!existsSync(filePath)) {
      // Return minimal state if file doesn't exist
      return {
        session: new Date().toISOString().split('T')[0],
        project: 'japan-travel',
        version: '3.0',
        active_destination: this.plan?.active_destination || '',
        current_focus: '',
        event_log: [],
        global_processes: {},
        destinations: {},
      };
    }
    const content = readFileSync(filePath, 'utf-8');
    return JSON.parse(content) as EventLogState;
  }

  /**
   * Save both travel plan and event log atomically.
   * Note: Does NOT update last_cascade_run - that is cascade-runner-owned.
   */
  save(): void {
    // Save travel plan
    writeFileSync(
      this.planPath,
      JSON.stringify(this.plan, null, 2),
      'utf-8'
    );

    // Save event log
    writeFileSync(
      this.statePath,
      JSON.stringify(this.eventLog, null, 2),
      'utf-8'
    );
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
