/**
 * Turso Repository
 *
 * Implements StateRepository reading itinerary data from normalized tables
 * (itinerary_days, itinerary_sessions, activities) instead of the JSON blob.
 *
 * Phase 2: reads from normalized tables, writes to both tables + blob.
 * Non-itinerary data (offers, packages, transport config) still comes from blob
 * via an internal BlobBridgeRepository delegate.
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
} from './types';
import type { StateRepository, DateAnchorData, ActivitySearchResult } from './repository';
import { BlobBridgeRepository, createBlobBridgeFromDb } from './blob-bridge-repository';

// ============================================================================
// Pipeline helper — same pattern as turso-service.ts
// ============================================================================

function requirePipeline(): { TursoPipelineClient: new (opts?: any) => any } {
  const path = require('node:path');
  return require(path.resolve(__dirname, '..', '..', 'scripts', 'turso-pipeline'));
}

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

// ============================================================================
// SQL helpers
// ============================================================================

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
// TursoRepository
// ============================================================================

export class TursoRepository implements StateRepository {
  /**
   * Internal BlobBridge for:
   * - Non-normalized data (offers, packages, transport config)
   * - Backward-compat blob writes on save()
   * - In-memory plan state (mutated by writes, written to blob on save)
   */
  private bridge: BlobBridgeRepository;
  private planId: string;

  private constructor(planId: string, bridge: BlobBridgeRepository) {
    this.planId = planId;
    this.bridge = bridge;
  }

  /**
   * Factory: create TursoRepository from DB.
   *
   * 1. Loads plan blob (for non-itinerary data + backward compat)
   * 2. Loads itinerary from normalized tables
   * 3. Overlays normalized data onto in-memory plan (tables are source of truth)
   */
  static async create(planId: string): Promise<TursoRepository> {
    const bridge = await createBlobBridgeFromDb(planId);
    const repo = new TursoRepository(planId, bridge);

    // Overlay itinerary from normalized tables onto in-memory plan
    await repo.loadNormalizedItinerary();

    console.error(`  [turso] TursoRepository ready for "${planId}" (normalized table reads)`);
    return repo;
  }

  // ==========================================================================
  // Normalized table loading
  // ==========================================================================

  /**
   * Read itinerary_days, itinerary_sessions, and activities from normalized
   * tables and overlay them onto the in-memory plan. This makes normalized
   * tables the source of truth for itinerary data.
   */
  private async loadNormalizedItinerary(): Promise<void> {
    const { TursoPipelineClient } = requirePipeline();
    const client = new TursoPipelineClient();

    // Load days, sessions, activities in parallel pipeline
    const [daysResp, sessionsResp, activitiesResp] = await Promise.all([
      client.execute(
        `SELECT * FROM itinerary_days WHERE plan_id = ${sqlText(this.planId)} ORDER BY destination, day_number`
      ),
      client.execute(
        `SELECT * FROM itinerary_sessions WHERE plan_id = ${sqlText(this.planId)} ORDER BY destination, day_number, session_type`
      ),
      client.execute(
        `SELECT * FROM activities WHERE plan_id = ${sqlText(this.planId)} ORDER BY destination, day_number, session_type, sort_order`
      ),
    ]);

    const dayRows = rowsToObjects(daysResp);
    const sessionRows = rowsToObjects(sessionsResp);
    const activityRows = rowsToObjects(activitiesResp);

    // If no normalized data exists, fall back to blob (first migration hasn't run)
    if (dayRows.length === 0) {
      console.error(`  [turso] No normalized itinerary data for "${this.planId}", using blob`);
      return;
    }

    // Group by destination
    const plan = this.bridge.getPlan();
    const destDays = new Map<string, Record<string, any>[]>();
    for (const row of dayRows) {
      const dest = row.destination;
      if (!destDays.has(dest)) destDays.set(dest, []);
      destDays.get(dest)!.push(row);
    }

    // Index sessions and activities by composite key
    const sessionKey = (dest: string, day: number, session: string) => `${dest}:${day}:${session}`;
    const sessionMap = new Map<string, Record<string, any>>();
    for (const row of sessionRows) {
      sessionMap.set(sessionKey(row.destination, row.day_number, row.session_type), row);
    }

    const activityMap = new Map<string, Record<string, any>[]>();
    for (const row of activityRows) {
      const key = sessionKey(row.destination, row.day_number, row.session_type);
      if (!activityMap.has(key)) activityMap.set(key, []);
      activityMap.get(key)!.push(row);
    }

    // Reconstruct day objects and overlay onto plan destinations
    for (const [destSlug, days] of destDays) {
      const destObj = plan.destinations[destSlug] as Record<string, unknown> | undefined;
      if (!destObj) continue;

      if (!destObj.process_5_daily_itinerary) {
        (destObj as Record<string, unknown>).process_5_daily_itinerary = {};
      }
      const p5 = destObj.process_5_daily_itinerary as Record<string, unknown>;

      const reconstructedDays: Record<string, unknown>[] = [];

      for (const dayRow of days) {
        const day: Record<string, unknown> = {
          day_number: dayRow.day_number,
          date: dayRow.date,
          theme: dayRow.theme,
          day_type: dayRow.day_type,
          status: dayRow.status || 'draft',
        };

        // Weather
        if (dayRow.weather_label || dayRow.temp_low_c !== null) {
          day.weather = {
            weather_label: dayRow.weather_label,
            temp_low_c: dayRow.temp_low_c !== null ? Number(dayRow.temp_low_c) : undefined,
            temp_high_c: dayRow.temp_high_c !== null ? Number(dayRow.temp_high_c) : undefined,
            precipitation_pct: dayRow.precipitation_pct !== null ? Number(dayRow.precipitation_pct) : undefined,
            weather_code: dayRow.weather_code !== null ? Number(dayRow.weather_code) : undefined,
            source_id: dayRow.weather_source_id,
            sourced_at: dayRow.weather_sourced_at,
          };
        }

        // Sessions
        for (const sessionType of ['morning', 'afternoon', 'evening'] as const) {
          const sKey = sessionKey(destSlug, dayRow.day_number, sessionType);
          const sRow = sessionMap.get(sKey);

          const session: Record<string, unknown> = {
            focus: sRow?.focus || null,
            transit_notes: sRow?.transit_notes || null,
            booking_notes: sRow?.booking_notes || null,
            activities: [],
          };

          if (sRow?.meals_json) {
            try { session.meals = JSON.parse(sRow.meals_json); } catch { /* ignore */ }
          }
          if (sRow?.time_range_start || sRow?.time_range_end) {
            session.time_range = {
              start: sRow?.time_range_start,
              end: sRow?.time_range_end,
            };
          }

          // Activities
          const acts = activityMap.get(sKey) || [];
          const activityObjects: Record<string, unknown>[] = [];
          for (const a of acts) {
            const actObj: Record<string, unknown> = {
              id: a.id,
              title: a.title,
              area: a.area || '',
              nearest_station: a.nearest_station || null,
              duration_min: a.duration_min !== null ? Number(a.duration_min) : null,
              booking_required: a.booking_required === 1 || a.booking_required === '1',
              booking_url: a.booking_url || null,
              booking_status: a.booking_status || undefined,
              booking_ref: a.booking_ref || undefined,
              book_by: a.book_by || undefined,
              start_time: a.start_time || undefined,
              end_time: a.end_time || undefined,
              is_fixed_time: a.is_fixed_time === 1 || a.is_fixed_time === '1',
              cost_estimate: a.cost_estimate !== null ? Number(a.cost_estimate) : null,
              notes: a.notes || null,
              priority: a.priority || 'want',
            };

            if (a.tags_json) {
              try { actObj.tags = JSON.parse(a.tags_json); } catch { actObj.tags = []; }
            } else {
              actObj.tags = [];
            }

            activityObjects.push(actObj);
          }

          session.activities = activityObjects;
          day[sessionType] = session;
        }

        reconstructedDays.push(day);
      }

      p5.days = reconstructedDays;
      console.error(`  [turso] Loaded ${reconstructedDays.length} days from normalized tables for ${destSlug}`);
    }
  }

  // ==========================================================================
  // StateReader — delegate to bridge (which now has normalized data in-memory)
  // ==========================================================================

  getActiveDestination(): string { return this.bridge.getActiveDestination(); }
  getSchemaVersion(): string { return this.bridge.getSchemaVersion(); }
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
  importOffers(dest: string, sourceId: string, offers: Array<Record<string, unknown>>, timestamp: string, note?: string, warnings?: string[]): void {
    this.bridge.importOffers(dest, sourceId, offers, timestamp, note, warnings);
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
