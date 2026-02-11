/**
 * Blob Bridge Repository
 *
 * Implements StateRepository over the existing JSON blob in plans.
 * All `Record<string, unknown>` casts and JSON path navigation live here.
 *
 * Phase 0: wraps current readPlanFromDb/writePlanToDb.
 * Phase 1-2: replaced by TursoRepository reading normalized tables.
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
  isValidProcessStatus,
} from './types';
import type { StateRepository, DateAnchorData, ActivitySearchResult } from './repository';
import {
  validateTravelPlan,
  validateEventLogState,
  validateDestinationSections,
  formatSectionValidationErrors,
} from './schemas';

export class BlobBridgeRepository implements StateRepository {
  private version: number;

  constructor(
    private plan: TravelPlanMinimal,
    private eventLog: EventLogState,
    version: number = 0,
  ) {
    this.version = version;
  }

  // ============================================================================
  // StateReader — Plan metadata
  // ============================================================================

  getActiveDestination(): string {
    return this.plan.active_destination;
  }

  getSchemaVersion(): string {
    return this.plan.schema_version;
  }

  getVersion(): number {
    return this.version;
  }

  // ============================================================================
  // StateReader — Process status
  // ============================================================================

  getProcessStatus(dest: string, process: ProcessId): ProcessStatus | null {
    const destObj = this.plan.destinations[dest];
    if (!destObj || !destObj[process]) return null;
    const processObj = destObj[process] as Record<string, unknown>;
    const raw = processObj['status'];
    if (!raw || typeof raw !== 'string') return null;
    return raw as ProcessStatus;
  }

  // ============================================================================
  // StateReader — Date anchor
  // ============================================================================

  getDateAnchor(dest: string): DateAnchorData | null {
    const destObj = this.plan.destinations[dest];
    if (!destObj) return null;

    const p1 = destObj.process_1_date_anchor as Record<string, unknown> | undefined;
    const dates = p1?.confirmed_dates as { start: string; end: string } | undefined;
    const days = p1?.days as number | undefined;

    if (!dates) return null;
    return { start: dates.start, end: dates.end, days: days || 0 };
  }

  // ============================================================================
  // StateReader — Cascade state
  // ============================================================================

  getCascadeState(): CascadeState {
    return this.plan.cascade_state;
  }

  isDirty(dest: string, process: ProcessId): boolean {
    const destState = this.plan.cascade_state.destinations[dest];
    return destState?.[process]?.dirty ?? false;
  }

  // ============================================================================
  // StateReader — Itinerary
  // ============================================================================

  getDay(dest: string, dayNumber: number): Record<string, unknown> | null {
    const destObj = this.plan.destinations[dest];
    if (!destObj) return null;

    const p5 = destObj.process_5_daily_itinerary as Record<string, unknown> | undefined;
    const days = p5?.days as Array<Record<string, unknown>> | undefined;
    if (!days) return null;

    return days.find(d => d.day_number === dayNumber) || null;
  }

  getDays(dest: string): Array<Record<string, unknown>> {
    const destObj = this.plan.destinations[dest];
    if (!destObj) return [];

    const p5 = destObj.process_5_daily_itinerary as Record<string, unknown> | undefined;
    return (p5?.days as Array<Record<string, unknown>>) || [];
  }

  getSessionActivities(dest: string, dayNumber: number, session: SessionType): Array<string | Record<string, unknown>> | null {
    const day = this.getDay(dest, dayNumber);
    if (!day) return null;

    const sessionObj = day[session] as { activities: Array<string | Record<string, unknown>> } | undefined;
    return sessionObj?.activities ?? null;
  }

  findActivityIndex(activities: Array<string | Record<string, unknown>>, idOrTitle: string): number {
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

  findActivity(dest: string, idOrTitle: string): ActivitySearchResult | null {
    const destObj = this.plan.destinations[dest];
    if (!destObj) return null;

    const p5 = destObj.process_5_daily_itinerary as Record<string, unknown> | undefined;
    const days = p5?.days as Array<Record<string, unknown>> | undefined;
    if (!days) return null;

    const sessions: SessionType[] = ['morning', 'afternoon', 'evening'];
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

  // ============================================================================
  // StateReader — Offers
  // ============================================================================

  getOffers(dest: string): Array<Record<string, unknown>> | null {
    const destObj = this.plan.destinations[dest];
    if (!destObj) return null;

    const packages = destObj.process_3_4_packages as Record<string, unknown> | undefined;
    const results = packages?.results as Record<string, unknown> | undefined;
    return (results?.offers as Array<Record<string, unknown>>) ?? null;
  }

  getOffer(dest: string, offerId: string): Record<string, unknown> | null {
    const offers = this.getOffers(dest);
    if (!offers) return null;
    return offers.find(o => o.id === offerId) ?? null;
  }

  // ============================================================================
  // StateReader — Event log
  // ============================================================================

  getEvents(): TravelEvent[] {
    return this.eventLog.event_log;
  }

  getNextActions(): string[] {
    return this.eventLog.next_actions || [];
  }

  // ============================================================================
  // StateReader — Raw plan access
  // ============================================================================

  getPlan(): TravelPlanMinimal {
    return this.plan;
  }

  getEventLog(): EventLogState {
    return this.eventLog;
  }

  // ============================================================================
  // StateWriter — Plan metadata
  // ============================================================================

  setActiveDestination(dest: string): void {
    this.plan.cascade_state.global.active_destination_last = this.plan.active_destination;
    this.plan.active_destination = dest;
  }

  // ============================================================================
  // StateWriter — Process status
  // ============================================================================

  setProcessStatusData(dest: string, process: ProcessId, status: ProcessStatus, timestamp: string): void {
    const destObj = this.plan.destinations[dest];
    if (destObj && destObj[process]) {
      const processObj = destObj[process] as Record<string, unknown>;
      processObj['status'] = status;
      processObj['updated_at'] = timestamp;
    }

    // Update event log state
    this.ensureEventLogDestination(dest);
    const destLog = this.eventLog.destinations[dest];
    if (!destLog.processes[process]) {
      destLog.processes[process] = { state: status, events: [] };
    }
    destLog.processes[process].state = status;
  }

  // ============================================================================
  // StateWriter — Date anchor
  // ============================================================================

  setDateAnchorData(dest: string, start: string, end: string, days: number, timestamp: string): void {
    const destObj = this.plan.destinations[dest];
    if (!destObj) throw new Error(`Destination not found: ${dest}`);

    if (!destObj.process_1_date_anchor) {
      (destObj as Record<string, unknown>).process_1_date_anchor = {};
    }
    const dateAnchor = destObj.process_1_date_anchor as Record<string, unknown>;
    dateAnchor.confirmed_dates = { start, end };
    dateAnchor.days = days;
    dateAnchor.updated_at = timestamp;
  }

  // ============================================================================
  // StateWriter — Cascade state
  // ============================================================================

  setDirtyFlag(dest: string, process: ProcessId, dirty: boolean, timestamp: string): void {
    if (!this.plan.cascade_state.destinations[dest]) {
      this.plan.cascade_state.destinations[dest] = {};
    }
    this.plan.cascade_state.destinations[dest][process] = {
      dirty,
      last_changed: timestamp,
    };
  }

  setGlobalDirtyFlag(process: 'process_1_date_anchor', dirty: boolean, timestamp: string): void {
    this.plan.cascade_state.global[process] = {
      dirty,
      last_changed: timestamp,
    };
  }

  markCascadeRun(timestamp: string): void {
    this.plan.cascade_state.last_cascade_run = timestamp;
  }

  // ============================================================================
  // StateWriter — Itinerary scaffolding
  // ============================================================================

  setDays(dest: string, days: Array<Record<string, unknown>>, timestamp: string): void {
    const destObj = this.plan.destinations[dest];
    if (!destObj) throw new Error(`Destination not found: ${dest}`);

    if (!destObj.process_5_daily_itinerary) {
      (destObj as Record<string, unknown>).process_5_daily_itinerary = {};
    }

    const p5 = destObj.process_5_daily_itinerary as Record<string, unknown>;
    p5.days = days;
    p5.updated_at = timestamp;
    p5.scaffolded_at = timestamp;
  }

  touchItinerary(dest: string, timestamp: string): void {
    const destObj = this.plan.destinations[dest];
    if (!destObj) return;

    const p5 = destObj.process_5_daily_itinerary as Record<string, unknown> | undefined;
    if (p5) {
      p5.updated_at = timestamp;
    }
  }

  // ============================================================================
  // StateWriter — Day-level mutations
  // ============================================================================

  setDayField(dest: string, dayNumber: number, field: string, value: unknown): void {
    const day = this.getDay(dest, dayNumber);
    if (!day) throw new Error(`Day ${dayNumber} not found in ${dest}`);
    (day as Record<string, unknown>)[field] = value;
  }

  // ============================================================================
  // StateWriter — Session-level mutations
  // ============================================================================

  setSessionField(dest: string, dayNumber: number, session: SessionType, field: string, value: unknown): void {
    const day = this.getDay(dest, dayNumber);
    if (!day) throw new Error(`Day ${dayNumber} not found in ${dest}`);

    const sessionObj = day[session] as Record<string, unknown> | undefined;
    if (!sessionObj) throw new Error(`Session ${session} not found in Day ${dayNumber}`);

    sessionObj[field] = value;
  }

  // ============================================================================
  // StateWriter — Activity mutations
  // ============================================================================

  addActivityToSession(dest: string, dayNumber: number, session: SessionType, activity: Record<string, unknown>): void {
    const day = this.getDay(dest, dayNumber);
    if (!day) throw new Error(`Day ${dayNumber} not found in ${dest}`);

    const sessionObj = day[session] as { activities: Array<Record<string, unknown>> };
    if (!sessionObj || !Array.isArray(sessionObj.activities)) {
      throw new Error(`Session ${session} not found in Day ${dayNumber}`);
    }

    sessionObj.activities.push(activity);
  }

  updateActivityAtIndex(
    dest: string, dayNumber: number, session: SessionType, index: number,
    updates: Record<string, unknown>
  ): Record<string, unknown> {
    const activities = this.getSessionActivities(dest, dayNumber, session);
    if (!activities) throw new Error(`Session ${session} not found in Day ${dayNumber}`);

    const current = activities[index];
    const activityObj = typeof current === 'string'
      ? this.upgradeStringActivity(current, { booking_required: false })
      : current as Record<string, unknown>;

    activities[index] = activityObj;
    Object.assign(activityObj, updates);
    return activityObj;
  }

  replaceActivityAtIndex(dest: string, dayNumber: number, session: SessionType, index: number, activity: Record<string, unknown>): void {
    const activities = this.getSessionActivities(dest, dayNumber, session);
    if (!activities) throw new Error(`Session ${session} not found in Day ${dayNumber}`);
    activities[index] = activity;
  }

  removeActivityAtIndex(dest: string, dayNumber: number, session: SessionType, index: number): string | Record<string, unknown> {
    const activities = this.getSessionActivities(dest, dayNumber, session);
    if (!activities) throw new Error(`Session ${session} not found in Day ${dayNumber}`);
    return activities.splice(index, 1)[0];
  }

  // ============================================================================
  // StateWriter — Offer mutations
  // ============================================================================

  setOfferAvailability(dest: string, offerId: string, date: string, data: Record<string, unknown>): { previousAvailability: unknown } {
    const offer = this.getOffer(dest, offerId);
    if (!offer) throw new Error(`Offer not found: ${offerId}`);

    let datePricing = offer.date_pricing as Record<string, Record<string, unknown>> | undefined;
    if (!datePricing) {
      datePricing = {};
      offer.date_pricing = datePricing;
    }

    const previousEntry = datePricing[date];
    const previousAvailability = previousEntry?.availability;

    datePricing[date] = {
      ...previousEntry,
      ...data,
    };

    return { previousAvailability };
  }

  setOfferSelection(dest: string, offerId: string, date: string, timestamp: string): Record<string, unknown> {
    const destObj = this.plan.destinations[dest];
    if (!destObj) throw new Error(`Destination not found: ${dest}`);

    const packages = destObj.process_3_4_packages as Record<string, unknown> | undefined;
    const results = packages?.results as Record<string, unknown> | undefined;
    const offers = results?.offers as Array<Record<string, unknown>> | undefined;
    if (!offers || !packages) throw new Error(`No offers found`);

    const offer = offers.find(o => o.id === offerId);
    if (!offer) throw new Error(`Offer not found: ${offerId}`);

    packages.selected_offer_id = offerId;
    packages.chosen_offer = { id: offerId, selected_date: date, selected_at: timestamp };
    if (!packages.results || typeof packages.results !== 'object') packages.results = {};
    (packages.results as Record<string, unknown>).chosen_offer = offer;

    return offer;
  }

  importOffers(dest: string, sourceId: string, offers: Array<Record<string, unknown>>, timestamp: string, note?: string, warnings?: string[]): void {
    const destObj = this.plan.destinations[dest];
    if (!destObj) throw new Error(`Destination not found: ${dest}`);

    if (!destObj.process_3_4_packages) {
      (destObj as Record<string, unknown>).process_3_4_packages = {};
    }

    const p34 = destObj.process_3_4_packages as Record<string, unknown>;
    if (!p34.results || typeof p34.results !== 'object') p34.results = {};
    const results = p34.results as Record<string, unknown>;

    results.offers = offers;
    const provenance = (results.provenance as Array<Record<string, unknown>> | undefined) ?? [];
    provenance.push({
      source_id: sourceId,
      scraped_at: timestamp,
      offers_found: offers.length,
      ...(note ? { note } : {}),
    });
    results.provenance = provenance;

    if (warnings && warnings.length > 0) {
      const existing = (results.warnings as string[] | undefined) ?? [];
      results.warnings = [...existing, ...warnings];
    }
  }

  populateFromOffer(dest: string, offer: Record<string, unknown>, date: string, timestamp: string): void {
    const destObj = this.plan.destinations[dest];
    if (!destObj) return;

    // Populate P3 (transportation) from offer flight info
    const flight = offer.flight as Record<string, unknown> | undefined;
    if (flight) {
      const p3 = destObj.process_3_transportation as Record<string, unknown> | undefined;
      if (p3) {
        p3.populated_from = `package:${offer.id}`;
        p3.flight = { ...flight, booked_date: date, populated_at: timestamp };
      }
    }

    // Populate P4 (accommodation) from offer hotel info
    const hotel = offer.hotel as Record<string, unknown> | undefined;
    if (hotel) {
      const p4 = destObj.process_4_accommodation as Record<string, unknown> | undefined;
      if (p4) {
        p4.populated_from = `package:${offer.id}`;
        p4.hotel = { ...hotel, check_in: date, populated_at: timestamp };
      }
    }
  }

  // ============================================================================
  // StateWriter — Transport mutations
  // ============================================================================

  ensureTransportationProcess(dest: string, timestamp: string): void {
    const destObj = this.plan.destinations[dest];
    if (!destObj) throw new Error(`Destination not found: ${dest}`);

    if (!destObj.process_3_transportation) {
      (destObj as Record<string, unknown>).process_3_transportation = {
        status: 'pending',
        updated_at: timestamp,
      };
    }

    const p3 = destObj.process_3_transportation as Record<string, unknown>;
    if (typeof p3.status !== 'string') {
      p3.status = 'pending';
    }
  }

  setAirportTransfer(dest: string, direction: 'arrival' | 'departure', segment: unknown, timestamp: string): void {
    this.ensureTransportationProcess(dest, timestamp);
    const destObj = this.plan.destinations[dest];
    const p3 = destObj.process_3_transportation as Record<string, unknown>;

    if (!p3.airport_transfers || typeof p3.airport_transfers !== 'object') {
      p3.airport_transfers = {};
    }

    (p3.airport_transfers as Record<string, unknown>)[direction] = segment as Record<string, unknown>;
  }

  addAirportTransferCandidate(dest: string, direction: 'arrival' | 'departure', option: TransportOption, timestamp: string): void {
    this.ensureTransportationProcess(dest, timestamp);
    const destObj = this.plan.destinations[dest];
    const p3 = destObj.process_3_transportation as Record<string, unknown>;

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
  }

  selectAirportTransferOption(dest: string, direction: 'arrival' | 'departure', optionId: string, _timestamp: string): TransportOption {
    const destObj = this.plan.destinations[dest];
    if (!destObj) throw new Error(`Destination not found: ${dest}`);

    const p3 = destObj.process_3_transportation as Record<string, unknown> | undefined;
    if (!p3?.airport_transfers || typeof p3.airport_transfers !== 'object') {
      throw new Error(`No airport transfers set for ${dest}`);
    }

    const transfers = p3.airport_transfers as Record<string, unknown>;
    const segment = transfers[direction] as Record<string, unknown> | undefined;
    if (!segment) throw new Error(`No ${direction} airport transfer segment found`);

    const candidates = (segment.candidates as TransportOption[] | undefined) ?? [];
    const selected = candidates.find(c => c.id === optionId);
    if (!selected) throw new Error(`Airport transfer option not found: ${optionId}`);

    segment.selected = selected;
    transfers[direction] = segment;

    return selected;
  }

  touchTransportation(dest: string, timestamp: string): void {
    const destObj = this.plan.destinations[dest];
    if (!destObj) return;

    const p3 = destObj.process_3_transportation as Record<string, unknown> | undefined;
    if (p3) {
      p3.updated_at = timestamp;
    }
  }

  // ============================================================================
  // StateWriter — Event log mutations
  // ============================================================================

  pushEvent(event: TravelEvent): void {
    this.eventLog.event_log.push(event);

    // Add to destination-specific log if applicable
    if (event.destination && event.process) {
      this.ensureEventLogDestination(event.destination);
      const destLog = this.eventLog.destinations[event.destination];
      if (!destLog.processes[event.process]) {
        destLog.processes[event.process] = { state: 'pending', events: [] };
      }
      destLog.processes[event.process].events.push(event);
    }
  }

  ensureEventLogDestination(dest: string): void {
    if (!this.eventLog.destinations[dest]) {
      this.eventLog.destinations[dest] = {
        status: 'active',
        processes: {},
      };
    }
  }

  setEventLogProcessState(dest: string, process: ProcessId, state: ProcessStatus): void {
    this.ensureEventLogDestination(dest);
    const destLog = this.eventLog.destinations[dest];
    if (!destLog.processes[process]) {
      destLog.processes[process] = { state, events: [] };
    }
    destLog.processes[process].state = state;
  }

  setEventLogActiveDestination(dest: string): void {
    this.eventLog.active_destination = dest;
  }

  setEventLogFocus(focus: string): void {
    this.eventLog.current_focus = focus;
  }

  setNextActions(actions: string[]): void {
    this.eventLog.next_actions = actions;
  }

  // ============================================================================
  // Persistence
  // ============================================================================

  async save(planId: string, schemaVersion: string): Promise<void> {
    // Validate before saving
    const sectionErrors: string[] = [];
    for (const [destSlug, destObj] of Object.entries(this.plan.destinations || {})) {
      const result = validateDestinationSections(destSlug, destObj as Record<string, unknown>);
      if (!result.valid) {
        sectionErrors.push(formatSectionValidationErrors(result));
      }
    }
    if (sectionErrors.length > 0) {
      throw new Error(`Section validation failed:\n${sectionErrors.join('\n')}`);
    }

    validateTravelPlan(this.plan);
    validateEventLogState(this.eventLog);

    const planJson = JSON.stringify(this.plan, null, 2);
    const stateJson = JSON.stringify(this.eventLog, null, 2);

    try {
      const { writePlanToDb } = require('../services/turso-service');
      await writePlanToDb(planId, planJson, stateJson, schemaVersion);
    } catch (e: any) {
      throw new Error(`DB write failed — save aborted: ${e.message}`);
    }

    // Blocking: sync normalized tables (TursoRepository reads from these)
    await this.syncNormalizedTables(planId);

    // Fire-and-forget: bookings + events (not on read-critical path)
    this.syncDerivedData(planId);
  }

  private syncDerivedData(planId: string): void {
    try {
      const { syncBookingsFromPlanJson, syncEventsToDb } = require('../services/turso-service');

      syncBookingsFromPlanJson(this.plan as unknown as Record<string, unknown>, planId).catch((e: Error) => {
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
   * Phase 1 dual-write: extract itinerary data from in-memory plan and
   * write to normalized tables (itinerary_days, itinerary_sessions, activities,
   * plan_metadata, date_anchors, process_statuses, cascade_dirty_flags,
   * airport_transfers, flights, hotels).
   *
   * Blocking — TursoRepository reads from these tables, so they must be
   * consistent before save() returns.
   */
  private async syncNormalizedTables(planId: string): Promise<void> {
    const { executePipelineTransaction, executePipelineRollback } = require('../services/turso-service');
    const statements: string[] = [];

    // Delete stale rows first — prevents ghost data when days/activities are removed
    const escapedPlanId = planId.replace(/'/g, "''");
    statements.push(
      `DELETE FROM activities WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM itinerary_sessions WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM itinerary_days WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM process_statuses WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM cascade_dirty_flags WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM airport_transfers WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM flights WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM hotels WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM date_anchors WHERE plan_id = '${escapedPlanId}'`,
    );

    // plan_metadata
    statements.push(
      `INSERT OR REPLACE INTO plan_metadata (plan_id, schema_version, active_destination, updated_at)
       VALUES (${sqlText(planId)}, ${sqlText(this.plan.schema_version)}, ${sqlText(this.plan.active_destination)}, datetime('now'))`
    );

    const destinations = this.plan.destinations;
    if (!destinations) return;

    for (const [destSlug, dest] of Object.entries(destinations)) {
      const destObj = dest as Record<string, unknown>;

      // date_anchors
      const p1 = destObj.process_1_date_anchor as Record<string, unknown> | undefined;
      if (p1) {
        const dates = p1.confirmed_dates as { start: string; end: string } | undefined;
        if (dates) {
          statements.push(
            `INSERT OR REPLACE INTO date_anchors (plan_id, destination, start_date, end_date, days, updated_at)
             VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlText(dates.start)}, ${sqlText(dates.end)}, ${sqlInt(p1.days as number)}, datetime('now'))`
          );
        }
      }

      // process_statuses
      const processIds = [
        'process_1_date_anchor', 'process_2_destination', 'process_3_4_packages',
        'process_3_transportation', 'process_4_accommodation', 'process_5_daily_itinerary',
      ];
      for (const pid of processIds) {
        const proc = destObj[pid] as Record<string, unknown> | undefined;
        if (proc?.status) {
          statements.push(
            `INSERT OR REPLACE INTO process_statuses (plan_id, destination, process_id, status, updated_at)
             VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlText(pid)}, ${sqlText(proc.status as string)}, datetime('now'))`
          );
        }
      }

      // airport_transfers + flights
      const p3 = destObj.process_3_transportation as Record<string, unknown> | undefined;
      if (p3?.airport_transfers && typeof p3.airport_transfers === 'object') {
        const transfers = p3.airport_transfers as Record<string, unknown>;
        for (const dir of ['arrival', 'departure'] as const) {
          const segment = transfers[dir] as Record<string, unknown> | undefined;
          if (segment) {
            statements.push(
              `INSERT OR REPLACE INTO airport_transfers (plan_id, destination, direction, status, selected_json, candidates_json, updated_at)
               VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlText(dir)}, ${sqlText((segment.status as string) || 'planned')}, ${sqlText(segment.selected ? JSON.stringify(segment.selected) : null)}, ${sqlText(segment.candidates ? JSON.stringify(segment.candidates) : null)}, datetime('now'))`
            );
          }
        }
      }

      if (p3?.flight && typeof p3.flight === 'object') {
        const flight = p3.flight as Record<string, unknown>;
        statements.push(
          `INSERT OR REPLACE INTO flights (plan_id, destination, populated_from, airline, airline_code, outbound_json, return_json, booked_date, updated_at)
           VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlText(p3.populated_from as string)}, ${sqlText(flight.airline as string)}, ${sqlText(flight.airline_code as string)}, ${sqlText(flight.outbound ? JSON.stringify(flight.outbound) : null)}, ${sqlText(flight.return ? JSON.stringify(flight.return) : null)}, ${sqlText(flight.booked_date as string)}, datetime('now'))`
        );
      }

      // hotels
      const p4 = destObj.process_4_accommodation as Record<string, unknown> | undefined;
      if (p4?.hotel && typeof p4.hotel === 'object') {
        const hotel = p4.hotel as Record<string, unknown>;
        statements.push(
          `INSERT OR REPLACE INTO hotels (plan_id, destination, populated_from, name, access_json, check_in, notes, updated_at)
           VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlText(p4.populated_from as string)}, ${sqlText(hotel.name as string)}, ${sqlText(hotel.access ? JSON.stringify(hotel.access) : null)}, ${sqlText(hotel.check_in as string)}, ${sqlText(hotel.notes as string)}, datetime('now'))`
        );
      }

      // cascade_dirty_flags
      const cascadeState = this.plan.cascade_state;
      const destFlags = cascadeState?.destinations?.[destSlug];
      if (destFlags) {
        for (const [pid, flag] of Object.entries(destFlags)) {
          const f = flag as { dirty: boolean; last_changed: string | null };
          statements.push(
            `INSERT OR REPLACE INTO cascade_dirty_flags (plan_id, destination, process_id, dirty, last_changed)
             VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlText(pid)}, ${sqlBool(f.dirty)}, ${sqlText(f.last_changed)})`
          );
        }
      }

      // itinerary_days + sessions + activities
      const p5 = destObj.process_5_daily_itinerary as Record<string, unknown> | undefined;
      const days = p5?.days as Array<Record<string, unknown>> | undefined;
      if (days && Array.isArray(days)) {
        for (const day of days) {
          const dayNumber = day.day_number as number;
          const weather = day.weather as Record<string, unknown> | undefined;

          statements.push(
            `INSERT OR REPLACE INTO itinerary_days (plan_id, destination, day_number, date, theme, day_type, status, weather_label, temp_low_c, temp_high_c, precipitation_pct, weather_code, weather_source_id, weather_sourced_at, updated_at)
             VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlInt(dayNumber)}, ${sqlText(day.date as string)}, ${sqlText(day.theme as string)}, ${sqlText(day.day_type as string)}, ${sqlText((day.status as string) || 'draft')}, ${sqlText(weather?.weather_label as string)}, ${sqlReal(weather?.temp_low_c as number)}, ${sqlReal(weather?.temp_high_c as number)}, ${sqlReal(weather?.precipitation_pct as number)}, ${sqlInt(weather?.weather_code as number)}, ${sqlText(weather?.source_id as string)}, ${sqlText(weather?.sourced_at as string)}, datetime('now'))`
          );

          for (const sessionType of ['morning', 'afternoon', 'evening'] as const) {
            const session = day[sessionType] as Record<string, unknown> | undefined;
            if (!session) continue;

            const timeRange = session.time_range as { start: string; end: string } | undefined;
            const meals = session.meals as string[] | undefined;

            statements.push(
              `INSERT OR REPLACE INTO itinerary_sessions (plan_id, destination, day_number, session_type, focus, transit_notes, booking_notes, meals_json, time_range_start, time_range_end, updated_at)
               VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlInt(dayNumber)}, ${sqlText(sessionType)}, ${sqlText(session.focus as string)}, ${sqlText(session.transit_notes as string)}, ${sqlText(session.booking_notes as string)}, ${sqlText(meals ? JSON.stringify(meals) : null)}, ${sqlText(timeRange?.start)}, ${sqlText(timeRange?.end)}, datetime('now'))`
            );

            const activities = session.activities as Array<string | Record<string, unknown>> | undefined;
            if (activities) {
              for (let i = 0; i < activities.length; i++) {
                const act = activities[i];

                if (typeof act === 'string') {
                  const actId = `migrated_${planId}_${destSlug}_d${dayNumber}_${sessionType}_${i}`;
                  statements.push(
                    `INSERT OR REPLACE INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title, priority, updated_at)
                     VALUES (${sqlText(actId)}, ${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlInt(dayNumber)}, ${sqlText(sessionType)}, ${sqlInt(i)}, ${sqlText(act)}, 'want', datetime('now'))`
                  );
                } else {
                  const actId = (act.id as string) || `migrated_${planId}_${destSlug}_d${dayNumber}_${sessionType}_${i}`;
                  statements.push(
                    `INSERT OR REPLACE INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title, area, nearest_station, duration_min, booking_required, booking_url, booking_status, booking_ref, book_by, start_time, end_time, is_fixed_time, cost_estimate, tags_json, notes, priority, updated_at)
                     VALUES (${sqlText(actId)}, ${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlInt(dayNumber)}, ${sqlText(sessionType)}, ${sqlInt(i)}, ${sqlText(act.title as string)}, ${sqlText(act.area as string)}, ${sqlText(act.nearest_station as string)}, ${sqlInt(act.duration_min as number)}, ${sqlBool(act.booking_required as boolean)}, ${sqlText(act.booking_url as string)}, ${sqlText(act.booking_status as string)}, ${sqlText(act.booking_ref as string)}, ${sqlText(act.book_by as string)}, ${sqlText(act.start_time as string)}, ${sqlText(act.end_time as string)}, ${sqlBool(act.is_fixed_time as boolean)}, ${sqlInt(act.cost_estimate as number)}, ${sqlText(act.tags ? JSON.stringify(act.tags) : null)}, ${sqlText(act.notes as string)}, ${sqlText((act.priority as string) || 'want')}, datetime('now'))`
                  );
                }
              }
            }
          }
        }
      }
    }

    if (statements.length > 0) {
      // Bump version as monotonic counter (audit trail, no conflict detection)
      statements.push(
        `UPDATE plans SET version = version + 1 WHERE plan_id = '${escapedPlanId}'`
      );

      // Wrap in transaction — single pipeline request preserves transaction scope
      statements.unshift('BEGIN');
      statements.push('COMMIT');
      try {
        await executePipelineTransaction(statements);
      } catch (e: any) {
        await executePipelineRollback();
        throw e;
      }

      this.version += 1;
    }
  }

  // ============================================================================
  // Internal helpers
  // ============================================================================

  /** Upgrade a legacy string activity to a structured object. */
  private upgradeStringActivity(title: string, overrides?: Partial<Record<string, unknown>>): Record<string, unknown> {
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
}

// ============================================================================
// SQL helpers for normalized table sync
// ============================================================================

function rowsToObjects(response: any): Record<string, any>[] {
  const result = response?.results?.[0]?.response?.result;
  if (!result?.rows || !result?.cols) return [];
  const cols = result.cols.map((c: any) => c.name);
  return result.rows.map((row: any[]) => {
    const obj: Record<string, any> = {};
    for (let i = 0; i < cols.length; i++) {
      const cell = row[i];
      obj[cols[i]] = cell?.value ?? null;
    }
    return obj;
  });
}

function sqlText(v: string | null | undefined): string {
  if (v === null || v === undefined) return 'NULL';
  return `'${String(v).replace(/'/g, "''")}'`;
}

function sqlInt(v: number | null | undefined): string {
  if (v === null || v === undefined) return 'NULL';
  return String(Math.round(v));
}

function sqlReal(v: number | null | undefined): string {
  if (v === null || v === undefined) return 'NULL';
  return String(v);
}

function sqlBool(v: boolean | null | undefined): string {
  if (v === null || v === undefined) return '0';
  return v ? '1' : '0';
}

// ============================================================================
// Factory: create BlobBridgeRepository from DB
// ============================================================================

/**
 * Load plan + state from Turso and return a BlobBridgeRepository.
 */
export async function createBlobBridgeFromDb(planId: string): Promise<BlobBridgeRepository> {
  const { readPlanFromDb } = require('../services/turso-service');

  let dbRow: { plan_json: string; state_json: string | null; updated_at: string; version: number } | null;
  try {
    dbRow = await readPlanFromDb(planId);
  } catch (e: any) {
    throw new Error(`[turso] DB read failed for plan "${planId}": ${e.message}`);
  }

  if (!dbRow) {
    throw new Error(`[turso] Plan "${planId}" not found in DB. Run 'npm run db:seed:plans' first.`);
  }

  let plan: TravelPlanMinimal;
  let eventLog: EventLogState | undefined;
  try {
    plan = JSON.parse(dbRow.plan_json) as TravelPlanMinimal;
    eventLog = dbRow.state_json ? JSON.parse(dbRow.state_json) as EventLogState : undefined;
  } catch (e: any) {
    throw new Error(`[turso] Invalid JSON in plans for plan "${planId}": ${e.message}`);
  }

  const version = dbRow.version ?? 0;
  console.error(`  [turso] Loaded plan "${planId}" from DB (updated: ${dbRow.updated_at}, version: ${version})`);

  if (!eventLog) {
    const { DEFAULTS } = require('../config/constants');
    eventLog = {
      session: new Date().toISOString().split('T')[0],
      project: DEFAULTS.project,
      version: '3.0',
      active_destination: plan.active_destination || '',
      current_focus: '',
      event_log: [],
      global_processes: {},
      destinations: {},
    };
  }

  return new BlobBridgeRepository(plan, eventLog, version);
}
