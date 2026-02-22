/**
 * In-Memory Plan Repository
 *
 * Implements StateRepository over an in-memory plan object.
 * TursoRepository assembles plan from normalized tables, then wraps it
 * in this class for mutation + write-back via syncNormalizedTables().
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
  FlightInfo,
  FlightLeg,
  HotelInfo,
  AirportTransfers,
  isValidProcessStatus,
} from './types';
import type { StateRepository, DateAnchorData, ActivitySearchResult, FlightLegInput, HotelInput } from './repository';
import {
  validateTravelPlan,
  validateEventLogState,
  validateDestinationSections,
  formatSectionValidationErrors,
} from './schemas';
import { sqlText, sqlInt, sqlReal, sqlBool } from './sql-helpers';

export class PlanRepository implements StateRepository {
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
  // StateReader — Flight / Hotel / Transport
  // ============================================================================

  getFlightInfo(dest: string): FlightInfo | null {
    const destObj = this.plan.destinations[dest];
    if (!destObj) return null;

    const p3 = destObj.process_3_transportation as Record<string, unknown> | undefined;
    const flight = p3?.flight as Record<string, unknown> | undefined;
    if (!flight) return null;

    const extractLeg = (raw: Record<string, unknown> | undefined | null): FlightLeg | null => {
      if (!raw) return null;
      return {
        flight_number: (raw.flight_number as string) ?? null,
        departure_airport_code: (raw.departure_airport_code as string) ?? null,
        departure_terminal: (raw.departure_terminal as string) ?? null,
        departure_time: (raw.departure_time as string) ?? null,
        arrival_airport_code: (raw.arrival_airport_code as string) ?? null,
        arrival_terminal: (raw.arrival_terminal as string) ?? null,
        arrival_time: (raw.arrival_time as string) ?? null,
        date: (raw.date as string) ?? null,
      };
    };

    return {
      airline: (flight.airline as string) ?? null,
      airline_code: (flight.airline_code as string) ?? null,
      booked_date: (flight.booked_date as string) ?? null,
      outbound: extractLeg(flight.outbound as Record<string, unknown> | undefined),
      return: extractLeg(flight.return as Record<string, unknown> | undefined),
    };
  }

  getHotelInfo(dest: string): HotelInfo | null {
    const destObj = this.plan.destinations[dest];
    if (!destObj) return null;

    const p4 = destObj.process_4_accommodation as Record<string, unknown> | undefined;
    const hotel = p4?.hotel as Record<string, unknown> | undefined;
    if (!hotel) return null;

    const access = hotel.access;
    return {
      name: (hotel.name as string) ?? null,
      access: Array.isArray(access) ? access as string[] : [],
      check_in: (hotel.check_in as string) ?? null,
      notes: (hotel.notes as string) ?? null,
    };
  }

  getAirportTransfers(dest: string): AirportTransfers | null {
    const destObj = this.plan.destinations[dest];
    if (!destObj) return null;

    const p3 = destObj.process_3_transportation as Record<string, unknown> | undefined;
    const transfers = p3?.airport_transfers as Record<string, unknown> | undefined;
    if (!transfers) return null;

    return transfers as unknown as AirportTransfers;
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

  importOffers(dest: string, sourceId: string, offers: Array<Record<string, unknown>>, timestamp: string, note?: string, warnings?: string[], filePath?: string, offerCount?: number): void {
    const destObj = this.plan.destinations[dest];
    if (!destObj) throw new Error(`Destination not found: ${dest}`);

    if (!destObj.process_3_4_packages) {
      (destObj as Record<string, unknown>).process_3_4_packages = {};
    }

    const p34 = destObj.process_3_4_packages as Record<string, unknown>;
    if (!p34.results || typeof p34.results !== 'object') p34.results = {};
    const results = p34.results as Record<string, unknown>;

    // Merge by offer ID (preserve offers from other sources)
    const existing = (results.offers as Array<Record<string, unknown>> | undefined) ?? [];
    const byId = new Map(existing.map(o => [o.id as string, o]));
    for (const o of offers) { byId.set(o.id as string, o); }
    results.offers = Array.from(byId.values());

    const provenance = (results.provenance as Array<Record<string, unknown>> | undefined) ?? [];
    provenance.push({
      source_id: sourceId,
      scraped_at: timestamp,
      offers_found: offerCount ?? offers.length,
      ...(note ? { note } : {}),
      ...(filePath ? { file_path: filePath } : {}),
      ...(offerCount !== undefined ? { offer_count: offerCount } : {}),
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

  setFlightLeg(dest: string, direction: 'outbound' | 'return', input: FlightLegInput, timestamp: string): void {
    const destObj = this.plan.destinations[dest];
    if (!destObj) throw new Error(`Destination not found: ${dest}`);

    this.ensureTransportationProcess(dest, timestamp);
    const p3 = destObj.process_3_transportation as Record<string, unknown>;

    if (!p3.flight || typeof p3.flight !== 'object') p3.flight = {};
    const flight = p3.flight as Record<string, unknown>;

    // Update shared flight-level fields
    if (input.airline !== undefined) flight.airline = input.airline;
    if (input.airlineCode !== undefined) flight.airline_code = input.airlineCode;
    if (input.bookedDate !== undefined) flight.booked_date = input.bookedDate;

    // Update leg-level fields
    if (!flight[direction] || typeof flight[direction] !== 'object') flight[direction] = {};
    const leg = flight[direction] as Record<string, unknown>;

    if (input.flightNumber !== undefined) leg.flight_number = input.flightNumber;
    if (input.departureCode !== undefined) leg.departure_airport_code = input.departureCode;
    if (input.departureTerminal !== undefined) leg.departure_terminal = input.departureTerminal;
    if (input.departureTime !== undefined) leg.departure_time = input.departureTime;
    if (input.arrivalCode !== undefined) leg.arrival_airport_code = input.arrivalCode;
    if (input.arrivalTerminal !== undefined) leg.arrival_terminal = input.arrivalTerminal;
    if (input.arrivalTime !== undefined) leg.arrival_time = input.arrivalTime;
    if (input.date !== undefined) leg.date = input.date;
  }

  setHotel(dest: string, input: HotelInput, timestamp: string): void {
    const destObj = this.plan.destinations[dest];
    if (!destObj) throw new Error(`Destination not found: ${dest}`);

    if (!destObj.process_4_accommodation) {
      (destObj as Record<string, unknown>).process_4_accommodation = {};
    }
    const p4 = destObj.process_4_accommodation as Record<string, unknown>;

    if (!p4.hotel || typeof p4.hotel !== 'object') p4.hotel = {};
    const hotel = p4.hotel as Record<string, unknown>;

    if (input.name !== undefined) hotel.name = input.name;
    if (input.access !== undefined) hotel.access = input.access;
    if (input.checkIn !== undefined) hotel.check_in = input.checkIn;
    if (input.notes !== undefined) hotel.notes = input.notes;
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

    // Write to normalized tables only (no blob)
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
   * airport_transfers, flight_legs, hotels).
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
      `DELETE FROM flight_legs WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM hotels WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM date_anchors WHERE plan_id = '${escapedPlanId}'`,
      // Phase 4: new normalized tables
      `DELETE FROM plan_destinations WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM destination_details WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM destination_cities WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM plan_offers WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM plan_offer_flights WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM plan_offer_hotels WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM plan_offer_date_pricing WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM plan_offer_best_value WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM plan_offer_selection WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM plan_offer_provenance WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM plan_offer_warnings WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM plan_budget WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM cascade_triggers WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM plan_schema_contract WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM plan_process_precedence WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM cascade_global_state WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM plan_root_date_anchor WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM itinerary_metadata WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM accommodation_location_zone WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM transportation_extras WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM event_log_state WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM event_log_global_processes WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM event_log_destinations WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM event_log_dest_processes WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM event_log_process_events WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM airport_transfer_candidates WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM hotel_access_lines WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM session_meals WHERE plan_id = '${escapedPlanId}'`,
      `DELETE FROM day_route_segments WHERE plan_id = '${escapedPlanId}'`,
    );

    // plan_metadata
    statements.push(
      `INSERT OR REPLACE INTO plan_metadata (plan_id, schema_version, active_destination, updated_at)
       VALUES (${sqlText(planId)}, ${sqlText(this.plan.schema_version)}, ${sqlText(this.plan.active_destination)}, datetime('now'))`
    );

    // Root-level normalized tables
    const plan = this.plan as any;
    if (plan.budget) {
      const b = plan.budget;
      statements.push(`INSERT INTO plan_budget (plan_id, total_cap, flight_cap, accommodation_cap, daily_cap, pax, currency) VALUES (${sqlText(planId)}, ${sqlInt(b.total_cap)}, ${sqlInt(b.flight_cap)}, ${sqlInt(b.accommodation_cap)}, ${sqlInt(b.daily_cap)}, ${sqlInt(b.pax ?? 1)}, ${sqlText(b.currency ?? 'TWD')})`);
    }
    if (plan.schema_contract) {
      const sc = plan.schema_contract;
      statements.push(`INSERT INTO plan_schema_contract (plan_id, id_convention, currency, process_nodes_json) VALUES (${sqlText(planId)}, ${sqlText(sc.id_convention)}, ${sqlText(sc.currency ?? 'TWD')}, ${sqlText(JSON.stringify(sc.process_nodes))})`);
    }
    if (plan.process_precedence) {
      statements.push(`INSERT INTO plan_process_precedence (plan_id, precedence_json) VALUES (${sqlText(planId)}, ${sqlText(JSON.stringify(plan.process_precedence))})`);
    }
    if (plan.cascade_rules?.triggers) {
      for (const t of plan.cascade_rules.triggers) {
        statements.push(`INSERT INTO cascade_triggers (plan_id, trigger_id, event, reset_json, scope, condition_json, action, populate_map_json, set_source) VALUES (${sqlText(planId)}, ${sqlText(t.id)}, ${sqlText(t.trigger)}, ${sqlText(JSON.stringify(t.reset))}, ${sqlText(t.scope)}, ${sqlText(t.condition ? JSON.stringify(t.condition) : null)}, ${sqlText(t.action)}, ${sqlText(t.populate_map ? JSON.stringify(t.populate_map) : null)}, ${sqlText(t.set_source)})`);
      }
    }
    if (plan.process_1_date_anchor) {
      const p1r = plan.process_1_date_anchor;
      statements.push(`INSERT INTO plan_root_date_anchor (plan_id, status, set_out_date, duration_days, return_date, flexibility_json) VALUES (${sqlText(planId)}, ${sqlText(p1r.status)}, ${sqlText(p1r.set_out_date)}, ${sqlInt(p1r.duration_days)}, ${sqlText(p1r.return_date)}, ${sqlText(p1r.flexibility ? JSON.stringify(p1r.flexibility) : null)})`);
    }
    if (plan.cascade_state) {
      const cs = plan.cascade_state;
      statements.push(`INSERT INTO cascade_global_state (plan_id, last_cascade_run, active_dest_last, p1_dirty) VALUES (${sqlText(planId)}, ${sqlText(cs.last_cascade_run)}, ${sqlText(cs.global?.active_destination_last)}, ${sqlBool(cs.global?.process_1_date_anchor?.dirty)})`);
    }

    // Event log
    const el = this.eventLog;
    statements.push(`INSERT INTO event_log_state (plan_id, session, project, version, current_focus, active_destination, next_actions_json) VALUES (${sqlText(planId)}, ${sqlText(el.session)}, ${sqlText(el.project)}, ${sqlText(el.version)}, ${sqlText(el.current_focus)}, ${sqlText(el.active_destination)}, ${sqlText(el.next_actions ? JSON.stringify(el.next_actions) : null)})`);
    for (const [pid, pobj] of Object.entries(el.global_processes || {})) {
      statements.push(`INSERT INTO event_log_global_processes (plan_id, process_id, status, events_json) VALUES (${sqlText(planId)}, ${sqlText(pid)}, ${sqlText(pobj.state)}, ${sqlText(JSON.stringify(pobj.events))})`);
    }
    for (const [dest, dobj] of Object.entries(el.destinations || {})) {
      statements.push(`INSERT INTO event_log_destinations (plan_id, destination, status) VALUES (${sqlText(planId)}, ${sqlText(dest)}, ${sqlText(dobj.status)})`);
      for (const [pid, pobj] of Object.entries(dobj.processes || {})) {
        statements.push(`INSERT INTO event_log_dest_processes (plan_id, destination, process_id, status, events_json) VALUES (${sqlText(planId)}, ${sqlText(dest)}, ${sqlText(pid)}, ${sqlText(pobj.state)}, ${sqlText(JSON.stringify(pobj.events))})`);
      }
    }
    for (const evt of el.event_log || []) {
      statements.push(`INSERT INTO event_log_process_events (plan_id, destination, process_id, event_type, event_data, event_at) VALUES (${sqlText(planId)}, ${sqlText(evt.destination)}, ${sqlText(evt.process ?? 'global')}, ${sqlText(evt.event)}, ${sqlText(evt.data ? JSON.stringify(evt.data) : null)}, ${sqlText(evt.at)})`);
    }

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

      // airport_transfers + flight_legs
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
        // Write normalized flight_legs rows (one per direction)
        for (const dir of ['outbound', 'return'] as const) {
          const leg = flight[dir] as Record<string, unknown> | null | undefined;
          if (!leg) continue;
          statements.push(
            `INSERT OR REPLACE INTO flight_legs (plan_id, destination, direction, leg_order, flight_number, airline, airline_code, departure_airport, departure_code, departure_terminal, departure_time, arrival_airport, arrival_code, arrival_terminal, arrival_time, flight_date, populated_from, booked_date, updated_at)
             VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlText(dir)}, 0, ${sqlText(leg.flight_number as string)}, ${sqlText(flight.airline as string)}, ${sqlText(flight.airline_code as string)}, ${sqlText(leg.departure_airport as string)}, ${sqlText(leg.departure_airport_code as string)}, ${sqlText(leg.departure_terminal as string)}, ${sqlText(leg.departure_time as string)}, ${sqlText(leg.arrival_airport as string)}, ${sqlText(leg.arrival_airport_code as string)}, ${sqlText(leg.arrival_terminal as string)}, ${sqlText(leg.arrival_time as string)}, ${sqlText(leg.date as string)}, ${sqlText(p3.populated_from as string)}, ${sqlText(flight.booked_date as string)}, datetime('now'))`
          );
        }
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

      // --- Phase 4: new per-destination tables ---

      // plan_destinations
      statements.push(`INSERT INTO plan_destinations (plan_id, slug, display_name, status, created_at, updated_at) VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlText((destObj as any).display_name ?? destSlug)}, ${sqlText((destObj as any).status ?? 'draft')}, ${sqlText((destObj as any).created_at)}, ${sqlText((destObj as any).updated_at)})`);

      // destination_details (P2)
      const p2 = destObj.process_2_destination as Record<string, unknown> | undefined;
      if (p2) {
        statements.push(`INSERT INTO destination_details (plan_id, destination, origin_city, region, primary_airport) VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlText(p2.origin_city as string)}, ${sqlText(p2.region as string)}, ${sqlText(p2.primary_airport as string)})`);
        const cities = p2.cities as Array<Record<string, unknown>> | undefined;
        if (cities) {
          for (const c of cities) {
            statements.push(`INSERT INTO destination_cities (plan_id, destination, city_slug, role, nights) VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlText(c.slug as string)}, ${sqlText(c.role as string)}, ${sqlInt(c.nights as number)})`);
          }
        }
      }

      // plan_offers (P3_4)
      const p34 = destObj.process_3_4_packages as Record<string, unknown> | undefined;
      const results34 = p34?.results as Record<string, unknown> | undefined;
      const offers = (results34?.offers || (p34 as any)?.offers || []) as Array<Record<string, unknown>>;
      for (const o of offers) {
        statements.push(`INSERT INTO plan_offers (plan_id, destination, id, source_id, type, title, price_per_person, currency, availability, url, scraped_at, product_code, duration_days, price_total, seats_remaining, includes_json) VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlText(o.id as string)}, ${sqlText(o.source_id as string)}, ${sqlText(o.type as string)}, ${sqlText(o.title as string)}, ${sqlInt(o.price_per_person as number)}, ${sqlText((o.currency as string) ?? 'TWD')}, ${sqlText(o.availability as string)}, ${sqlText(o.url as string)}, ${sqlText(o.scraped_at as string)}, ${sqlText(o.product_code as string)}, ${sqlInt(o.duration_days as number)}, ${sqlInt(o.price_total as number)}, ${sqlInt(o.seats_remaining as number)}, ${sqlText(o.includes ? JSON.stringify(o.includes) : null)})`);
        const oflight = o.flight as Record<string, unknown> | undefined;
        if (oflight) {
          for (const dir of ['outbound', 'return'] as const) {
            const leg = oflight[dir] as Record<string, unknown> | undefined;
            if (!leg) continue;
            statements.push(`INSERT INTO plan_offer_flights (plan_id, destination, offer_id, direction, flight_number, airline, airline_code, departure_code, departure_time, arrival_code, arrival_time) VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlText(o.id as string)}, ${sqlText(dir)}, ${sqlText(leg.flight_number as string)}, ${sqlText(oflight.airline as string)}, ${sqlText(oflight.airline_code as string)}, ${sqlText(leg.departure_airport_code as string)}, ${sqlText(leg.departure_time as string)}, ${sqlText(leg.arrival_airport_code as string)}, ${sqlText(leg.arrival_time as string)})`);
          }
        }
        const ohotel = o.hotel as Record<string, unknown> | undefined;
        if (ohotel) {
          statements.push(`INSERT INTO plan_offer_hotels (plan_id, destination, offer_id, name, slug, area, star_rating, access_json) VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlText(o.id as string)}, ${sqlText(ohotel.name as string)}, ${sqlText(ohotel.slug as string)}, ${sqlText(ohotel.area as string)}, ${sqlInt(ohotel.star_rating as number)}, ${sqlText(ohotel.access ? JSON.stringify(ohotel.access) : null)})`);
        }
        const dp = o.date_pricing as Record<string, Record<string, unknown>> | undefined;
        if (dp) {
          for (const [date, d] of Object.entries(dp)) {
            statements.push(`INSERT INTO plan_offer_date_pricing (plan_id, destination, offer_id, date, price, availability, seats_remaining) VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlText(o.id as string)}, ${sqlText(date)}, ${sqlInt(d.price as number)}, ${sqlText(d.availability as string)}, ${sqlInt(d.seats_remaining as number)})`);
          }
        }
      }

      // offer selection
      if (p34?.selected_offer_id || results34?.selection) {
        const sel = results34?.selection as Record<string, unknown> | undefined;
        statements.push(`INSERT INTO plan_offer_selection (plan_id, destination, selected_offer_id, selected_date, selected_at) VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlText((p34?.selected_offer_id || sel?.offer_id) as string)}, ${sqlText(sel?.date as string)}, ${sqlText((sel?.selected_at || p34?.updated_at) as string)})`);
      }

      // plan_offer_provenance
      const provenance = (results34?.provenance as Array<Record<string, unknown>> | undefined) ?? [];
      for (const prov of provenance) {
        if (prov.source_id && prov.scraped_at) {
          statements.push(`INSERT OR IGNORE INTO plan_offer_provenance (plan_id, destination, source_id, scraped_at, file_path, offer_count) VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlText(prov.source_id as string)}, ${sqlText(prov.scraped_at as string)}, ${sqlText(prov.file_path as string ?? null)}, ${sqlInt(prov.offer_count as number ?? null)})`);
        }
      }

      // accommodation_location_zone
      if (p4) {
        const lz = (p4 as any).location_zone;
        if (lz) {
          statements.push(`INSERT INTO accommodation_location_zone (plan_id, destination, selected_area, source, candidates_json) VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlText(lz.selected_area)}, ${sqlText(lz.source)}, ${sqlText(lz.candidates ? JSON.stringify(lz.candidates) : null)})`);
        }
      }

      // transportation_extras
      if (p3) {
        statements.push(`INSERT INTO transportation_extras (plan_id, destination, source, populated_from, home_to_airport_json, airport_to_hotel_json) VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlText(p3.source as string)}, ${sqlText(p3.populated_from as string)}, ${sqlText((p3 as any).home_to_airport ? JSON.stringify((p3 as any).home_to_airport) : null)}, ${sqlText((p3 as any).airport_to_hotel ? JSON.stringify((p3 as any).airport_to_hotel) : null)})`);
      }

      // airport_transfer_candidates + selected_* scalars
      if (p3?.airport_transfers && typeof p3.airport_transfers === 'object') {
        const transfers = p3.airport_transfers as Record<string, unknown>;
        for (const dir of ['arrival', 'departure'] as const) {
          const seg = transfers[dir] as Record<string, unknown> | undefined;
          if (!seg) continue;
          const sel = seg.selected as Record<string, unknown> | undefined;
          if (sel) {
            statements.push(`UPDATE airport_transfers SET selected_title = ${sqlText(sel.title as string)}, selected_route = ${sqlText(sel.route as string)}, selected_duration_min = ${sqlInt(sel.duration_min as number)}, selected_price_yen = ${sqlInt(sel.price_yen as number)}, selected_schedule = ${sqlText(sel.schedule as string)}, selected_booking_url = ${sqlText(sel.booking_url as string)}, selected_notes = ${sqlText(sel.notes as string)} WHERE plan_id = ${sqlText(planId)} AND destination = ${sqlText(destSlug)} AND direction = ${sqlText(dir)}`);
          }
          const cands = seg.candidates as Array<Record<string, unknown>> | undefined;
          if (cands) {
            for (let ci = 0; ci < cands.length; ci++) {
              const c = cands[ci];
              statements.push(`INSERT INTO airport_transfer_candidates (plan_id, destination, direction, candidate_id, title, route, duration_min, price_yen, schedule, booking_url, notes, sort_order) VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlText(dir)}, ${sqlText(c.id as string)}, ${sqlText((c.title || c.method) as string)}, ${sqlText(c.route as string)}, ${sqlInt(c.duration_min as number)}, ${sqlInt((c.price_yen ?? c.cost_jpy) as number)}, ${sqlText(c.schedule as string)}, ${sqlText(c.booking_url as string)}, ${sqlText(c.notes as string)}, ${sqlInt(ci)})`);
            }
          }
        }
      }

      // hotel_access_lines
      if (p4?.hotel && typeof p4.hotel === 'object') {
        const access = (p4.hotel as any).access;
        if (Array.isArray(access)) {
          for (let ai = 0; ai < access.length; ai++) {
            statements.push(`INSERT INTO hotel_access_lines (plan_id, destination, sort_order, line) VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlInt(ai)}, ${sqlText(access[ai])})`);
          }
        }
      }

      // itinerary_metadata (must be after p5 declaration)
      const p5 = destObj.process_5_daily_itinerary as Record<string, unknown> | undefined;
      if (p5) {
        statements.push(`INSERT INTO itinerary_metadata (plan_id, destination, scaffolded_at, populated_at, transit_summary) VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlText(p5.scaffolded_at as string)}, ${sqlText(p5.populated_at as string)}, ${sqlText(p5.transit_summary as string)})`);
      }

      // session_meals + activity_tags
      const days = p5?.days as Array<Record<string, unknown>> | undefined;
      if (days && Array.isArray(days)) {
        for (const day of days) {
          const dayNumber = day.day_number as number;
          const weather = day.weather as Record<string, unknown> | undefined;

          statements.push(
            `INSERT OR REPLACE INTO itinerary_days (plan_id, destination, day_number, date, theme, theme_zh, day_type, status, weather_label, temp_low_c, temp_high_c, feels_like_low_c, feels_like_high_c, precipitation_pct, weather_code, weather_source_id, weather_sourced_at, updated_at)
             VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlInt(dayNumber)}, ${sqlText(day.date as string)}, ${sqlText(day.theme as string)}, ${sqlText(day.theme_zh as string)}, ${sqlText(day.day_type as string)}, ${sqlText((day.status as string) || 'draft')}, ${sqlText(weather?.weather_label as string)}, ${sqlReal(weather?.temp_low_c as number)}, ${sqlReal(weather?.temp_high_c as number)}, ${sqlReal(weather?.feels_like_low_c as number)}, ${sqlReal(weather?.feels_like_high_c as number)}, ${sqlReal(weather?.precipitation_pct as number)}, ${sqlInt(weather?.weather_code as number)}, ${sqlText(weather?.source_id as string)}, ${sqlText(weather?.sourced_at as string)}, datetime('now'))`
          );

          for (const sessionType of ['morning', 'afternoon', 'evening'] as const) {
            const session = day[sessionType] as Record<string, unknown> | undefined;
            if (!session) continue;

            const timeRange = session.time_range as { start: string; end: string } | undefined;
            const meals = session.meals as string[] | undefined;
            const mealsZh = session.meals_zh as string[] | undefined | null;

            const activitiesZh = session.activities_zh as string[] | undefined | null;
            statements.push(
              `INSERT OR REPLACE INTO itinerary_sessions (plan_id, destination, day_number, session_type, focus, focus_zh, transit_notes, transit_notes_zh, activities_zh_json, booking_notes, meals_json, meals_zh_json, time_range_start, time_range_end, updated_at)
               VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlInt(dayNumber)}, ${sqlText(sessionType)}, ${sqlText(session.focus as string)}, ${sqlText(session.focus_zh as string)}, ${sqlText(session.transit_notes as string)}, ${sqlText(session.transit_notes_zh as string)}, ${sqlText(activitiesZh ? JSON.stringify(activitiesZh) : null)}, ${sqlText(session.booking_notes as string)}, ${sqlText(meals ? JSON.stringify(meals) : null)}, ${sqlText(mealsZh ? JSON.stringify(mealsZh) : null)}, ${sqlText(timeRange?.start)}, ${sqlText(timeRange?.end)}, datetime('now'))`
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
                  // activity_tags
                  const tags = act.tags as string[] | undefined;
                  if (tags && Array.isArray(tags)) {
                    for (const tag of tags) {
                      statements.push(`INSERT OR IGNORE INTO activity_tags (activity_id, tag) VALUES (${sqlText(actId)}, ${sqlText(tag)})`);
                    }
                  }
                }
              }
            }

            // session_meals
            if (meals && Array.isArray(meals)) {
              for (let mi = 0; mi < meals.length; mi++) {
                statements.push(`INSERT INTO session_meals (plan_id, destination, day_number, session_type, sort_order, meal) VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlInt(dayNumber)}, ${sqlText(sessionType)}, ${sqlInt(mi)}, ${sqlText(meals[mi])})`);
              }
            }
          }

          // route_segments
          const routeSegments = day.route_segments as Array<Record<string, unknown>> | undefined;
          if (routeSegments && Array.isArray(routeSegments)) {
            for (const seg of routeSegments) {
              statements.push(
                `INSERT OR REPLACE INTO day_route_segments (plan_id, destination, day_number, sort_order, from_place, to_place, mode, duration_min, notes, start_time)
                 VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlInt(dayNumber)}, ${sqlInt(seg.sort_order as number)}, ${sqlText(seg.from_place as string)}, ${sqlText(seg.to_place as string)}, ${sqlText(seg.mode as string)}, ${sqlInt(seg.duration_min as number)}, ${sqlText(seg.notes as string)}, ${sqlText(seg.start_time as string)})`
              );
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


