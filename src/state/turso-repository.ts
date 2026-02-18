/**
 * Turso Repository
 *
 * Implements StateRepository by reading all data from normalized tables.
 * Delegates in-memory mutations and write-back to PlanRepository.
 */

import type {
  ProcessId,
  ProcessStatus,
  SessionType,
  CascadeState,
  TravelEvent,
  TravelPlanMinimal,
  EventLogState,
  TransportOption,
  FlightInfo,
  HotelInfo,
  AirportTransfers,
} from './types';
import type { StateRepository, DateAnchorData, ActivitySearchResult, FlightLegInput, HotelInput } from './repository';
import { PlanRepository } from './plan-repository';
import { assemblePlan, assembleEventLog } from './plan-assembler';
import { rowsToObjectsAt, sqlText } from './sql-helpers';

// ============================================================================
// Pipeline helper — same pattern as turso-service.ts
// ============================================================================

function requirePipeline(): { TursoPipelineClient: new (opts?: any) => any } {
  const path = require('node:path');
  return require(path.resolve(__dirname, '..', '..', 'scripts', 'turso-pipeline'));
}

// ============================================================================
// TursoRepository
// ============================================================================

export class TursoRepository implements StateRepository {
  private bridge: PlanRepository;
  private planId: string;

  private constructor(planId: string, bridge: PlanRepository) {
    this.planId = planId;
    this.bridge = bridge;
  }

  /**
   * Factory: create TursoRepository from normalized tables (v5.0.0).
   * No blob fallback — tables are the sole source of truth.
   */
  static async create(planId: string): Promise<TursoRepository> {
    const { TursoPipelineClient } = requirePipeline();
    const client = new TursoPipelineClient();
    const pid = sqlText(planId);

    // Single batch: all table queries
    const queries = [
      `SELECT * FROM plan_metadata WHERE plan_id = ${pid}`,                                          // 0
      `SELECT * FROM plan_destinations WHERE plan_id = ${pid}`,                                      // 1
      `SELECT * FROM destination_details WHERE plan_id = ${pid}`,                                    // 2
      `SELECT * FROM destination_cities WHERE plan_id = ${pid} ORDER BY destination, city_slug`,     // 3
      `SELECT * FROM date_anchors WHERE plan_id = ${pid}`,                                           // 4
      `SELECT * FROM process_statuses WHERE plan_id = ${pid}`,                                       // 5
      `SELECT * FROM cascade_dirty_flags WHERE plan_id = ${pid}`,                                    // 6
      `SELECT * FROM plan_offers WHERE plan_id = ${pid}`,                                            // 7
      `SELECT * FROM plan_offer_flights WHERE plan_id = ${pid}`,                                     // 8
      `SELECT * FROM plan_offer_hotels WHERE plan_id = ${pid}`,                                      // 9
      `SELECT * FROM plan_offer_date_pricing WHERE plan_id = ${pid}`,                                // 10
      `SELECT * FROM plan_offer_selection WHERE plan_id = ${pid}`,                                   // 11
      `SELECT * FROM plan_budget WHERE plan_id = ${pid}`,                                            // 12
      `SELECT * FROM cascade_triggers WHERE plan_id = ${pid}`,                                       // 13
      `SELECT * FROM plan_schema_contract WHERE plan_id = ${pid}`,                                   // 14
      `SELECT * FROM plan_process_precedence WHERE plan_id = ${pid}`,                                // 15
      `SELECT * FROM cascade_global_state WHERE plan_id = ${pid}`,                                   // 16
      `SELECT * FROM plan_root_date_anchor WHERE plan_id = ${pid}`,                                  // 17
      `SELECT * FROM itinerary_days WHERE plan_id = ${pid} ORDER BY destination, day_number`,        // 18
      `SELECT * FROM itinerary_sessions WHERE plan_id = ${pid} ORDER BY destination, day_number`,    // 19
      `SELECT * FROM activities WHERE plan_id = ${pid} ORDER BY destination, day_number, session_type, sort_order`, // 20
      `SELECT * FROM flight_legs WHERE plan_id = ${pid} ORDER BY destination, direction, leg_order`, // 21
      `SELECT * FROM hotels WHERE plan_id = ${pid}`,                                                 // 22
      `SELECT * FROM airport_transfers WHERE plan_id = ${pid}`,                                      // 23
      `SELECT * FROM accommodation_location_zone WHERE plan_id = ${pid}`,                            // 24
      `SELECT * FROM transportation_extras WHERE plan_id = ${pid}`,                                  // 25
      `SELECT * FROM itinerary_metadata WHERE plan_id = ${pid}`,                                     // 26
      `SELECT * FROM airport_transfer_candidates WHERE plan_id = ${pid} ORDER BY destination, direction, sort_order`, // 27
      `SELECT * FROM hotel_access_lines WHERE plan_id = ${pid} ORDER BY destination, sort_order`,    // 28
      `SELECT * FROM session_meals WHERE plan_id = ${pid} ORDER BY destination, day_number, session_type, sort_order`, // 29
      `SELECT at.activity_id, at.tag FROM activity_tags at INNER JOIN activities a ON at.activity_id = a.id WHERE a.plan_id = ${pid}`, // 30
      `SELECT * FROM event_log_state WHERE plan_id = ${pid}`,                                        // 31
      `SELECT * FROM event_log_global_processes WHERE plan_id = ${pid}`,                             // 32
      `SELECT * FROM event_log_destinations WHERE plan_id = ${pid}`,                                 // 33
      `SELECT * FROM event_log_dest_processes WHERE plan_id = ${pid}`,                               // 34
      `SELECT * FROM event_log_process_events WHERE plan_id = ${pid} ORDER BY event_at`,             // 35
      `SELECT COALESCE(version, 0) as version FROM plans WHERE plan_id = ${pid}`,                    // 36
      `SELECT * FROM plan_offer_best_value WHERE plan_id = ${pid}`,                                  // 37
    ];

    // Execute all queries in a single HTTP round-trip
    const batchResponse = await client.executeBatch(queries);
    const responses: Record<string, any>[][] = queries.map((_, i) => rowsToObjectsAt(batchResponse, i));

    const [
      metaRows, destRows, detailRows, cityRows, dateRows, statusRows, dirtyRows,
      offerRows, offerFlightRows, offerHotelRows, offerDatePricingRows, offerSelRows,
      budgetRows, triggerRows, contractRows, precedenceRows, cascadeGlobalRows, rootP1Rows,
      dayRows, sessionRows, activityRows, flightLegRows, hotelRows, transferRows,
      locZoneRows, transportExtraRows, itinMetaRows, transferCandRows, accessLineRows,
      mealRows, tagRows, eventLogStateRows, eventLogGlobalRows, eventLogDestRows,
      eventLogDestProcRows, eventLogEvtRows, versionRows, bestValueRows,
    ] = responses;

    if (metaRows.length === 0) {
      throw new Error(`[turso] Plan "${planId}" not found in normalized tables. Run migration first.`);
    }

    // Assemble plan
    const plan = assemblePlan(
      planId, metaRows, destRows, detailRows, cityRows, dateRows, statusRows, dirtyRows,
      offerRows, offerFlightRows, offerHotelRows, offerDatePricingRows, offerSelRows,
      budgetRows, triggerRows, contractRows, precedenceRows, cascadeGlobalRows, rootP1Rows,
      dayRows, sessionRows, activityRows, flightLegRows, hotelRows, transferRows,
      locZoneRows, transportExtraRows, itinMetaRows, transferCandRows, accessLineRows,
      mealRows, tagRows, bestValueRows,
    );

    const eventLog = assembleEventLog(
      eventLogStateRows, eventLogGlobalRows, eventLogDestRows, eventLogDestProcRows, eventLogEvtRows,
    );

    const version = versionRows[0]?.version ? parseInt(versionRows[0].version, 10) : 0;
    const bridge = new PlanRepository(plan, eventLog, version);
    const repo = new TursoRepository(planId, bridge);

    console.error(`  [turso] TursoRepository ready for "${planId}" (fully normalized, v5.0.0)`);
    return repo;
  }

  // ==========================================================================
  // StateReader — delegate to bridge
  // ==========================================================================

  getActiveDestination(): string { return this.bridge.getActiveDestination(); }
  getSchemaVersion(): string { return this.bridge.getSchemaVersion(); }
  getVersion(): number { return this.bridge.getVersion(); }
  getProcessStatus(dest: string, process: ProcessId): ProcessStatus | null {
    return this.bridge.getProcessStatus(dest, process);
  }
  getDateAnchor(dest: string): DateAnchorData | null { return this.bridge.getDateAnchor(dest); }
  getCascadeState(): CascadeState { return this.bridge.getCascadeState(); }
  isDirty(dest: string, process: ProcessId): boolean { return this.bridge.isDirty(dest, process); }
  getDay(dest: string, dayNumber: number): Record<string, unknown> | null {
    return this.bridge.getDay(dest, dayNumber);
  }
  getDays(dest: string): Array<Record<string, unknown>> { return this.bridge.getDays(dest); }
  getSessionActivities(dest: string, dayNumber: number, session: SessionType): Array<string | Record<string, unknown>> | null {
    return this.bridge.getSessionActivities(dest, dayNumber, session);
  }
  findActivityIndex(activities: Array<string | Record<string, unknown>>, idOrTitle: string): number {
    return this.bridge.findActivityIndex(activities, idOrTitle);
  }
  findActivity(dest: string, idOrTitle: string): ActivitySearchResult | null {
    return this.bridge.findActivity(dest, idOrTitle);
  }
  getFlightInfo(dest: string): FlightInfo | null { return this.bridge.getFlightInfo(dest); }
  getHotelInfo(dest: string): HotelInfo | null { return this.bridge.getHotelInfo(dest); }
  getAirportTransfers(dest: string): AirportTransfers | null { return this.bridge.getAirportTransfers(dest); }
  getOffers(dest: string): Array<Record<string, unknown>> | null { return this.bridge.getOffers(dest); }
  getOffer(dest: string, offerId: string): Record<string, unknown> | null { return this.bridge.getOffer(dest, offerId); }
  getEvents(): TravelEvent[] { return this.bridge.getEvents(); }
  getNextActions(): string[] { return this.bridge.getNextActions(); }
  getPlan(): TravelPlanMinimal { return this.bridge.getPlan(); }
  getEventLog(): EventLogState { return this.bridge.getEventLog(); }

  // ==========================================================================
  // StateWriter — delegate to bridge (in-memory mutation + blob dual-write on save)
  // ==========================================================================

  setActiveDestination(dest: string): void { this.bridge.setActiveDestination(dest); }
  setProcessStatusData(dest: string, process: ProcessId, status: ProcessStatus, timestamp: string): void {
    this.bridge.setProcessStatusData(dest, process, status, timestamp);
  }
  setDateAnchorData(dest: string, start: string, end: string, days: number, timestamp: string): void {
    this.bridge.setDateAnchorData(dest, start, end, days, timestamp);
  }
  setDirtyFlag(dest: string, process: ProcessId, dirty: boolean, timestamp: string): void {
    this.bridge.setDirtyFlag(dest, process, dirty, timestamp);
  }
  setGlobalDirtyFlag(process: 'process_1_date_anchor', dirty: boolean, timestamp: string): void {
    this.bridge.setGlobalDirtyFlag(process, dirty, timestamp);
  }
  markCascadeRun(timestamp: string): void { this.bridge.markCascadeRun(timestamp); }
  setDays(dest: string, days: Array<Record<string, unknown>>, timestamp: string): void {
    this.bridge.setDays(dest, days, timestamp);
  }
  touchItinerary(dest: string, timestamp: string): void { this.bridge.touchItinerary(dest, timestamp); }
  setDayField(dest: string, dayNumber: number, field: string, value: unknown): void {
    this.bridge.setDayField(dest, dayNumber, field, value);
  }
  setSessionField(dest: string, dayNumber: number, session: SessionType, field: string, value: unknown): void {
    this.bridge.setSessionField(dest, dayNumber, session, field, value);
  }
  addActivityToSession(dest: string, dayNumber: number, session: SessionType, activity: Record<string, unknown>): void {
    this.bridge.addActivityToSession(dest, dayNumber, session, activity);
  }
  updateActivityAtIndex(
    dest: string, dayNumber: number, session: SessionType, index: number,
    updates: Record<string, unknown>
  ): Record<string, unknown> {
    return this.bridge.updateActivityAtIndex(dest, dayNumber, session, index, updates);
  }
  replaceActivityAtIndex(dest: string, dayNumber: number, session: SessionType, index: number, activity: Record<string, unknown>): void {
    this.bridge.replaceActivityAtIndex(dest, dayNumber, session, index, activity);
  }
  removeActivityAtIndex(dest: string, dayNumber: number, session: SessionType, index: number): string | Record<string, unknown> {
    return this.bridge.removeActivityAtIndex(dest, dayNumber, session, index);
  }
  setOfferAvailability(dest: string, offerId: string, date: string, data: Record<string, unknown>): { previousAvailability: unknown } {
    return this.bridge.setOfferAvailability(dest, offerId, date, data);
  }
  setOfferSelection(dest: string, offerId: string, date: string, timestamp: string): Record<string, unknown> {
    return this.bridge.setOfferSelection(dest, offerId, date, timestamp);
  }
  importOffers(dest: string, sourceId: string, offers: Array<Record<string, unknown>>, timestamp: string, note?: string, warnings?: string[], filePath?: string, offerCount?: number): void {
    this.bridge.importOffers(dest, sourceId, offers, timestamp, note, warnings, filePath, offerCount);
  }
  populateFromOffer(dest: string, offer: Record<string, unknown>, date: string, timestamp: string): void {
    this.bridge.populateFromOffer(dest, offer, date, timestamp);
  }
  ensureTransportationProcess(dest: string, timestamp: string): void {
    this.bridge.ensureTransportationProcess(dest, timestamp);
  }
  setAirportTransfer(dest: string, direction: 'arrival' | 'departure', segment: unknown, timestamp: string): void {
    this.bridge.setAirportTransfer(dest, direction, segment, timestamp);
  }
  addAirportTransferCandidate(dest: string, direction: 'arrival' | 'departure', option: TransportOption, timestamp: string): void {
    this.bridge.addAirportTransferCandidate(dest, direction, option, timestamp);
  }
  selectAirportTransferOption(dest: string, direction: 'arrival' | 'departure', optionId: string, timestamp: string): TransportOption {
    return this.bridge.selectAirportTransferOption(dest, direction, optionId, timestamp);
  }
  touchTransportation(dest: string, timestamp: string): void { this.bridge.touchTransportation(dest, timestamp); }
  setFlightLeg(dest: string, direction: 'outbound' | 'return', input: FlightLegInput, timestamp: string): void {
    this.bridge.setFlightLeg(dest, direction, input, timestamp);
  }
  setHotel(dest: string, input: HotelInput, timestamp: string): void {
    this.bridge.setHotel(dest, input, timestamp);
  }
  pushEvent(event: TravelEvent): void { this.bridge.pushEvent(event); }
  ensureEventLogDestination(dest: string): void { this.bridge.ensureEventLogDestination(dest); }
  setEventLogProcessState(dest: string, process: ProcessId, state: ProcessStatus): void {
    this.bridge.setEventLogProcessState(dest, process, state);
  }
  setEventLogActiveDestination(dest: string): void { this.bridge.setEventLogActiveDestination(dest); }
  setEventLogFocus(focus: string): void { this.bridge.setEventLogFocus(focus); }
  setNextActions(actions: string[]): void { this.bridge.setNextActions(actions); }

  // ==========================================================================
  // Persistence — blob + normalized tables (via bridge dual-write)
  // ==========================================================================

  async save(planId: string, schemaVersion: string): Promise<void> {
    return this.bridge.save(planId, schemaVersion);
  }
}
