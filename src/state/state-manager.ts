/**
 * State Manager
 *
 * Event-driven state machine for travel planning.
 * Handles validation, transitions, cascade, and event emission.
 * Delegates all data access to StateRepository (no direct JSON navigation).
 *
 * Usage:
 *   const sm = await StateManager.create();
 *   sm.dispatch({ type: 'set_day_theme', destination: 'tokyo_2026', dayNumber: 1, theme: 'Arrival' });
 *   await sm.save();
 */

import { readFileSync, existsSync, realpathSync } from 'fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {
  ProcessStatus,
  ProcessId,
  CascadeState,
  TravelEvent,
  EventLogState,
  TravelPlanMinimal,
  TravelState,
  STATUS_TRANSITIONS,
  TransportOption,
  TransportSegment,
  isValidProcessStatus,
} from './types';
import type { DayWeather, SessionType } from './types';
import {
  validateTravelPlan,
  validateEventLogState,
} from './schemas';
import { DEFAULTS, PATHS } from '../config/constants';
import { OfferManager } from './offer-manager';
import { TransportManager } from './transport-manager';
import { ItineraryManager } from './itinerary-manager';
import { EventQuery } from './event-query';
import type { StateRepository } from './repository';
import { BlobBridgeRepository, createBlobBridgeFromDb } from './blob-bridge-repository';
import { TursoRepository } from './turso-repository';
import type { Command, DispatchResult } from './commands';

// Default paths
const DEFAULT_PLAN_PATH = process.env.TRAVEL_PLAN_PATH || PATHS.defaultPlan;
const DEFAULT_STATE_PATH = process.env.TRAVEL_STATE_PATH || PATHS.defaultState;

/** SQL text literal helper — escapes single quotes, returns NULL for nullish values. */
function sqlTextFn(v: string | null | undefined): string {
  if (v == null) return 'NULL';
  return `'${String(v).replace(/'/g, "''")}'`;
}

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
  /** Pre-built repository (skips internal construction) */
  repo?: StateRepository;
}

export class StateManager {
  private planPath: string;
  private statePath: string;
  private planId: string;

  // Repository — all data access goes through here
  private repo: StateRepository;

  // Domain managers (legacy, operate on same plan object)
  private offerMgr: OfferManager;
  private transportMgr: TransportManager;
  private itineraryMgr: ItineraryManager;
  public events: EventQuery;
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
      const plan = this.loadPlanFromFile();
      this.normalizePlan(plan);
      const eventLog = this.loadEventLogFromFile(plan);
      this.repo = new BlobBridgeRepository(plan, eventLog);
    } else {
      // New options-based signature
      this.planPath = options.planPath || DEFAULT_PLAN_PATH;
      this.statePath = options.statePath || DEFAULT_STATE_PATH;
      this.planId = StateManager.derivePlanId(this.planPath);
      this.skipSave = options.skipSave || false;
      this.timestamp = this.freshTimestamp();

      if (options.repo) {
        // Pre-built repository (from factory methods)
        this.repo = options.repo;
      } else {
        let plan: TravelPlanMinimal;
        let eventLog: EventLogState;

        if (options.plan) {
          plan = options.plan;
          this.normalizePlan(plan);
        } else {
          plan = this.loadPlanFromFile();
          this.normalizePlan(plan);
        }

        if (options.eventLog) {
          eventLog = options.eventLog;
        } else if (options.state) {
          eventLog = {
            session: new Date().toISOString().split('T')[0],
            project: DEFAULTS.project,
            version: '3.0',
            active_destination: plan?.active_destination || '',
            current_focus: '',
            event_log: options.state.event_log || [],
            next_actions: options.state.next_actions || [],
            global_processes: {},
            destinations: {},
          };
        } else {
          eventLog = this.loadEventLogFromFile(plan);
        }

        this.repo = new BlobBridgeRepository(plan, eventLog);
      }
    }

    // Initialize domain managers (legacy — operate on same plan object by reference)
    const plan = this.repo.getPlan();
    this.offerMgr = new OfferManager(
      plan,
      () => this.timestamp,
      (e) => this.emitEvent(e),
      (d, p, s) => this.setProcessStatus(d, p, s),
      (d, p) => this.clearDirty(d, p)
    );
    this.transportMgr = new TransportManager(
      plan,
      () => this.timestamp,
      (e) => this.emitEvent(e)
    );
    this.itineraryMgr = new ItineraryManager(
      plan,
      () => this.timestamp,
      (e) => this.emitEvent(e),
      (d, p, s) => this.setProcessStatus(d, p, s),
      (d, p, s, data) => this.forceSetProcessStatus(d, p, s, data),
      (d, p) => this.clearDirty(d, p)
    );
    this.events = new EventQuery(() => this.repo.getEvents());
  }

  // ============================================================================
  // Factory (DB-primary)
  // ============================================================================

  /**
   * Derive a plan ID from the file path.
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

    const hash = crypto.createHash('sha1').update(canonicalPath).digest('hex').slice(0, 12);
    return `path:${hash}`;
  }

  /**
   * DB-only factory: read plan+state from Turso via repository.
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
    const repo = await TursoRepository.create(planId);

    return new StateManager({
      planPath: planPath,
      statePath: stPath,
      repo,
      skipSave: false,
    });
  }

  /**
   * DB-only factory by explicit plan ID.
   */
  static async createFromPlanId(planId: string, skipSave = false): Promise<StateManager> {
    if (!planId || typeof planId !== 'string') {
      throw new Error('planId is required');
    }

    if (skipSave) {
      // Fallback to file-based for test mode
      const planPath = path.join('data', 'trips', planId, 'travel-plan.json');
      const stPath = path.join('data', 'trips', planId, 'state.json');
      return new StateManager({ planPath, statePath: stPath, skipSave: true });
    }

    const repo = await TursoRepository.create(planId);

    const planPath = path.join('data', 'trips', planId, 'travel-plan.json');
    const stPath = path.join('data', 'trips', planId, 'state.json');
    const sm = new StateManager({
      planPath,
      statePath: stPath,
      repo,
      skipSave: false,
    });

    // Force plan ID to the explicit caller-provided DB key.
    sm.planId = planId;
    return sm;
  }

  // ============================================================================
  // Repository Access
  // ============================================================================

  /** Get the underlying repository (for advanced callers). */
  getRepository(): StateRepository {
    return this.repo;
  }

  // ============================================================================
  // Dispatch — command entry point
  // ============================================================================

  /**
   * Dispatch a command to the state machine.
   * This is the canonical entry point for state mutations.
   * Named methods below are convenience wrappers over dispatch().
   */
  dispatch(command: Command): DispatchResult {
    switch (command.type) {
      case 'set_date_anchor':
        this.setDateAnchor(command.startDate, command.endDate, command.reason);
        return {};

      case 'set_process_status':
        this.setProcessStatus(command.destination, command.process, command.status);
        return {};

      case 'mark_dirty':
        this.markDirty(command.destination, command.process);
        return {};

      case 'clear_dirty':
        this.clearDirty(command.destination, command.process);
        return {};

      case 'mark_global_dirty':
        this.markGlobalDirty(command.process);
        return {};

      case 'clear_global_dirty':
        this.clearGlobalDirty(command.process);
        return {};

      case 'set_active_destination':
        this.setActiveDestination(command.destination);
        return {};

      case 'set_focus':
        this.setFocus(command.destination, command.process);
        return {};

      case 'set_next_actions':
        this.setNextActions(command.actions);
        return {};

      case 'mark_cascade_run':
        this.markCascadeRun(command.timestamp);
        return {};

      case 'update_offer_availability':
        this.updateOfferAvailability(
          command.offerId, command.date, command.availability,
          command.price, command.seatsRemaining, command.source
        );
        return {};

      case 'select_offer':
        this.selectOffer(command.offerId, command.date, command.populateCascade);
        return {};

      case 'import_package_offers':
        this.importPackageOffers(
          command.destination, command.sourceId, command.offers,
          command.note, command.warnings
        );
        return {};

      case 'set_airport_transfer':
        this.setAirportTransferSegment(command.destination, command.direction, command.segment);
        return {};

      case 'add_airport_transfer_candidate':
        this.addAirportTransferCandidate(command.destination, command.direction, command.option);
        return {};

      case 'select_airport_transfer':
        this.selectAirportTransferOption(command.destination, command.direction, command.optionId);
        return {};

      case 'scaffold_itinerary':
        this.scaffoldItinerary(command.destination, command.days, command.force);
        return {};

      case 'add_activity': {
        const activityId = this.addActivity(
          command.destination, command.dayNumber, command.session, command.activity
        );
        return { activityId };
      }

      case 'update_activity':
        this.updateActivity(
          command.destination, command.dayNumber, command.session,
          command.activityId, command.updates
        );
        return {};

      case 'remove_activity':
        this.removeActivity(command.destination, command.dayNumber, command.session, command.activityId);
        return {};

      case 'set_activity_booking':
        this.setActivityBookingStatus(
          command.destination, command.dayNumber, command.session,
          command.activityIdOrTitle, command.status, command.ref, command.bookBy
        );
        return {};

      case 'set_activity_time':
        this.setActivityTime(
          command.destination, command.dayNumber, command.session,
          command.activityIdOrTitle,
          {
            start_time: command.startTime,
            end_time: command.endTime,
            is_fixed_time: command.isFixedTime,
          }
        );
        return {};

      case 'set_session_time_range':
        this.setSessionTimeRange(
          command.destination, command.dayNumber, command.session,
          command.start, command.end
        );
        return {};

      case 'set_day_theme':
        this.setDayTheme(command.destination, command.dayNumber, command.theme);
        return {};

      case 'set_day_weather':
        this.setDayWeather(command.destination, command.dayNumber, command.weather);
        return {};

      case 'set_session_focus':
        this.setSessionFocus(
          command.destination, command.dayNumber, command.session, command.focus
        );
        return {};

      default: {
        const _exhaustive: never = command;
        throw new Error(`Unknown command type: ${(command as any).type}`);
      }
    }
  }

  // ============================================================================
  // Timestamp
  // ============================================================================

  now(): string {
    return this.timestamp;
  }

  freshTimestamp(): string {
    return new Date().toISOString();
  }

  refreshTimestamp(): void {
    this.timestamp = this.freshTimestamp();
  }

  // ============================================================================
  // Cascade Run Tracking
  // ============================================================================

  markCascadeRun(timestamp?: string): void {
    this.repo.markCascadeRun(timestamp || this.timestamp);
  }

  getLastCascadeRun(): string {
    return this.repo.getCascadeState().last_cascade_run;
  }

  // ============================================================================
  // Dirty Flags
  // ============================================================================

  markDirty(destination: string, process: ProcessId): void {
    this.repo.setDirtyFlag(destination, process, true, this.timestamp);
    this.emitEvent({
      event: 'marked_dirty',
      destination,
      process,
      data: { dirty: true },
    });
  }

  clearDirty(destination: string, process: ProcessId): void {
    this.repo.setDirtyFlag(destination, process, false, this.timestamp);
  }

  markGlobalDirty(process: 'process_1_date_anchor'): void {
    this.repo.setGlobalDirtyFlag(process, true, this.timestamp);
    this.emitEvent({
      event: 'marked_global_dirty',
      process,
      data: { dirty: true },
    });
  }

  clearGlobalDirty(process: 'process_1_date_anchor'): void {
    this.repo.setGlobalDirtyFlag(process, false, this.timestamp);
  }

  getDirtyFlags(): CascadeState {
    return this.repo.getCascadeState();
  }

  isDirty(destination: string, process: ProcessId): boolean {
    return this.repo.isDirty(destination, process);
  }

  // ============================================================================
  // Process Status
  // ============================================================================

  setProcessStatus(
    destination: string,
    process: ProcessId,
    newStatus: ProcessStatus
  ): void {
    const currentStatus = this.getProcessStatus(destination, process);

    // Idempotent: allow setting the same status without emitting events.
    if (currentStatus === newStatus) {
      this.repo.setProcessStatusData(destination, process, newStatus, this.timestamp);
      return;
    }

    // Validate transition
    if (currentStatus && !this.isValidTransition(currentStatus, newStatus)) {
      throw new Error(
        `Invalid transition: ${currentStatus} → ${newStatus} for ${destination}.${process}`
      );
    }

    this.repo.setProcessStatusData(destination, process, newStatus, this.timestamp);

    this.emitEvent({
      event: 'status_changed',
      destination,
      process,
      from: currentStatus ?? undefined,
      to: newStatus,
    });
  }

  getProcessStatus(destination: string, process: ProcessId): ProcessStatus | null {
    const raw = this.repo.getProcessStatus(destination, process);
    if (raw === null) return null;
    if (!isValidProcessStatus(raw)) {
      throw new Error(
        `Invalid status "${raw}" in ${destination}.${process}. Valid: pending, researching, researched, selecting, selected, populated, booking, booked, confirmed, skipped`
      );
    }
    return raw;
  }

  isValidTransition(from: ProcessStatus, to: ProcessStatus): boolean {
    const allowed = STATUS_TRANSITIONS[from];
    return allowed?.includes(to) ?? false;
  }

  // ============================================================================
  // Event Logging
  // ============================================================================

  emitEvent(event: Omit<TravelEvent, 'at'>): void {
    const fullEvent: TravelEvent = {
      ...event,
      at: this.timestamp,
    };
    this.repo.pushEvent(fullEvent);
  }

  getEventLog(): TravelEvent[] {
    return this.repo.getEvents();
  }

  // ============================================================================
  // Date Anchor Management
  // ============================================================================

  setDateAnchor(startDate: string, endDate: string, reason?: string): void {
    const dest = this.getActiveDestination();

    // Get current dates for comparison
    const currentAnchor = this.repo.getDateAnchor(dest);
    const oldStart = currentAnchor?.start;
    const oldEnd = currentAnchor?.end;

    // Calculate days
    const start = new Date(startDate);
    const end = new Date(endDate);
    const days = Math.ceil((end.getTime() - start.getTime()) / (1000 * 60 * 60 * 24)) + 1;

    // Update via repo
    this.repo.setDateAnchorData(dest, startDate, endDate, days, this.timestamp);
    this.setProcessStatus(dest, 'process_1_date_anchor', 'confirmed');

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
        reason: reason || 'User updated dates',
      },
    });

    this.markGlobalDirty('process_1_date_anchor');

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

  getDateAnchor(): { start: string; end: string; days: number } | null {
    const dest = this.getActiveDestination();
    return this.repo.getDateAnchor(dest);
  }

  // ============================================================================
  // Offer Management
  // ============================================================================

  updateOfferAvailability(
    offerId: string,
    date: string,
    availability: 'available' | 'sold_out' | 'limited',
    price?: number,
    seatsRemaining?: number,
    source: string = 'user'
  ): void {
    const dest = this.getActiveDestination();

    const { previousAvailability } = this.repo.setOfferAvailability(dest, offerId, date, {
      availability,
      ...(price !== undefined && { price }),
      ...(seatsRemaining !== undefined && { seats_remaining: seatsRemaining }),
      note: `Updated by ${source} at ${this.timestamp}`,
    });

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

  selectOffer(offerId: string, date: string, populateCascade: boolean = true): void {
    const dest = this.getActiveDestination();
    const offer = this.repo.setOfferSelection(dest, offerId, date, this.timestamp);

    this.setProcessStatus(dest, 'process_3_4_packages', 'selected');

    this.emitEvent({
      event: 'offer_selected',
      destination: dest,
      process: 'process_3_4_packages',
      data: {
        offer_id: offerId,
        date,
        offer_name: (offer as Record<string, unknown>).name,
        hotel: ((offer as Record<string, unknown>).hotel as Record<string, unknown> | undefined)?.name,
        price_total: ((offer as Record<string, unknown>).date_pricing as Record<string, Record<string, unknown>> | undefined)?.[date]?.price,
      },
    });

    if (populateCascade) {
      this.populateFromOffer(dest, offer as Record<string, unknown>, date);
    }
  }

  private populateFromOffer(
    destination: string,
    offer: Record<string, unknown>,
    date: string
  ): void {
    this.repo.populateFromOffer(destination, offer, date, this.timestamp);

    // Set statuses for populated processes
    const flight = offer.flight as Record<string, unknown> | undefined;
    if (flight) {
      this.setProcessStatus(destination, 'process_3_transportation', 'populated');
      this.clearDirty(destination, 'process_3_transportation');
    }

    const hotel = offer.hotel as Record<string, unknown> | undefined;
    if (hotel) {
      this.setProcessStatus(destination, 'process_4_accommodation', 'populated');
      this.clearDirty(destination, 'process_4_accommodation');
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

  importPackageOffers(
    destination: string,
    sourceId: string,
    offers: Array<Record<string, unknown>>,
    note?: string,
    warnings?: string[]
  ): void {
    this.repo.importOffers(destination, sourceId, offers, this.timestamp, note, warnings);

    const currentStatus = this.getProcessStatus(destination, 'process_3_4_packages');
    if (!currentStatus || currentStatus === 'pending' || currentStatus === 'researching') {
      this.setProcessStatus(destination, 'process_3_4_packages', 'researched');
    } else {
      // Bump updated_at on packages process (status unchanged)
      this.repo.setProcessStatusData(destination, 'process_3_4_packages', currentStatus, this.timestamp);
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

  setAirportTransferSegment(
    destination: string,
    direction: 'arrival' | 'departure',
    segment: TransportSegment
  ): void {
    this.repo.setAirportTransfer(destination, direction, segment, this.timestamp);
    this.repo.touchTransportation(destination, this.timestamp);

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

  addAirportTransferCandidate(
    destination: string,
    direction: 'arrival' | 'departure',
    option: TransportOption
  ): void {
    this.repo.addAirportTransferCandidate(destination, direction, option, this.timestamp);
    this.repo.touchTransportation(destination, this.timestamp);

    this.emitEvent({
      event: 'airport_transfer_candidate_added',
      destination,
      process: 'process_3_transportation',
      data: { direction, option_id: option.id, title: option.title },
    });
  }

  selectAirportTransferOption(
    destination: string,
    direction: 'arrival' | 'departure',
    optionId: string
  ): void {
    const selected = this.repo.selectAirportTransferOption(destination, direction, optionId, this.timestamp);
    this.repo.touchTransportation(destination, this.timestamp);

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

  scaffoldItinerary(
    destination: string,
    days: Array<Record<string, unknown>>,
    force: boolean = false
  ): void {
    const currentStatus = this.getProcessStatus(destination, 'process_5_daily_itinerary');

    if (force && currentStatus && !['pending', 'researching'].includes(currentStatus)) {
      this.forceSetProcessStatus(destination, 'process_5_daily_itinerary', 'pending', {
        reason: 'force re-scaffold',
        from: currentStatus,
      });
    }

    this.repo.setDays(destination, days, this.timestamp);
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

  addActivity(
    destination: string,
    dayNumber: number,
    session: SessionType,
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
    const day = this.repo.getDay(destination, dayNumber);
    if (!day) throw new Error(`Day ${dayNumber} not found in ${destination}`);

    const activities = this.repo.getSessionActivities(destination, dayNumber, session);
    if (!activities) throw new Error(`Session ${session} not found in Day ${dayNumber}`);

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

    this.repo.addActivityToSession(destination, dayNumber, session, fullActivity);
    this.repo.touchItinerary(destination, this.timestamp);

    this.emitEvent({
      event: 'activity_added',
      destination,
      process: 'process_5_daily_itinerary',
      data: { day_number: dayNumber, session, activity_id: id, title: activity.title },
    });

    return id;
  }

  updateActivity(
    destination: string,
    dayNumber: number,
    session: SessionType,
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
    const activities = this.repo.getSessionActivities(destination, dayNumber, session);
    if (!activities) throw new Error(`Session ${session} not found in Day ${dayNumber}`);

    const idx = this.repo.findActivityIndex(activities, activityId);
    if (idx === -1) throw new Error(`Activity ${activityId} not found in Day ${dayNumber} ${session}`);

    const activityObj = this.repo.updateActivityAtIndex(
      destination, dayNumber, session, idx, updates as Record<string, unknown>
    );
    this.repo.touchItinerary(destination, this.timestamp);

    this.emitEvent({
      event: 'activity_updated',
      destination,
      process: 'process_5_daily_itinerary',
      data: { day_number: dayNumber, session, activity_id: activityObj.id, updates: Object.keys(updates) },
    });
  }

  setActivityTime(
    destination: string,
    dayNumber: number,
    session: SessionType,
    activityIdOrTitle: string,
    opts: { start_time?: string; end_time?: string; is_fixed_time?: boolean }
  ): void {
    const activities = this.repo.getSessionActivities(destination, dayNumber, session);
    if (!activities) {
      const day = this.repo.getDay(destination, dayNumber);
      if (!day) throw new Error(`Day ${dayNumber} not found in ${destination}`);
      throw new Error(`Session ${session} not found in Day ${dayNumber}`);
    }

    const idx = this.repo.findActivityIndex(activities, activityIdOrTitle);
    if (idx === -1) {
      throw new Error(`Activity not found: "${activityIdOrTitle}" in Day ${dayNumber} ${session}`);
    }

    // Get current values for diff
    const current = activities[idx];
    const currentObj = typeof current === 'string' ? {} : current as Record<string, unknown>;
    const previous = {
      start_time: currentObj.start_time as string | undefined,
      end_time: currentObj.end_time as string | undefined,
      is_fixed_time: currentObj.is_fixed_time as boolean | undefined,
    };

    // Build update object (only set fields that were provided)
    const updates: Record<string, unknown> = {};
    if (opts.start_time !== undefined) updates.start_time = opts.start_time;
    if (opts.end_time !== undefined) updates.end_time = opts.end_time;
    if (opts.is_fixed_time !== undefined) updates.is_fixed_time = opts.is_fixed_time;

    const activityObj = this.repo.updateActivityAtIndex(
      destination, dayNumber, session, idx, updates
    );
    this.repo.touchItinerary(destination, this.timestamp);

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

  setSessionTimeRange(
    destination: string,
    dayNumber: number,
    session: SessionType,
    start: string,
    end: string
  ): void {
    this.repo.setSessionField(destination, dayNumber, session, 'time_range', { start, end });
    this.repo.touchItinerary(destination, this.timestamp);

    this.emitEvent({
      event: 'session_time_range_updated',
      destination,
      process: 'process_5_daily_itinerary',
      data: { day_number: dayNumber, session, start, end },
    });
  }

  removeActivity(
    destination: string,
    dayNumber: number,
    session: SessionType,
    activityId: string
  ): void {
    const activities = this.repo.getSessionActivities(destination, dayNumber, session);
    if (!activities) {
      const day = this.repo.getDay(destination, dayNumber);
      if (!day) throw new Error(`Day ${dayNumber} not found in ${destination}`);
      throw new Error(`Session ${session} not found in Day ${dayNumber}`);
    }

    const idx = this.repo.findActivityIndex(activities, activityId);
    if (idx === -1) throw new Error(`Activity ${activityId} not found in Day ${dayNumber} ${session}`);

    const removed = this.repo.removeActivityAtIndex(destination, dayNumber, session, idx);
    this.repo.touchItinerary(destination, this.timestamp);

    this.emitEvent({
      event: 'activity_removed',
      destination,
      process: 'process_5_daily_itinerary',
      data: {
        day_number: dayNumber,
        session,
        activity_id: typeof removed === 'string' ? null : (removed as Record<string, unknown>).id,
        title: typeof removed === 'string' ? removed : ((removed as Record<string, unknown>).title as string | undefined),
      },
    });
  }

  setActivityBookingStatus(
    destination: string,
    dayNumber: number,
    session: SessionType,
    activityIdOrTitle: string,
    status: 'not_required' | 'pending' | 'booked' | 'waitlist',
    ref?: string,
    bookBy?: string
  ): void {
    const activities = this.repo.getSessionActivities(destination, dayNumber, session);
    if (!activities) {
      const day = this.repo.getDay(destination, dayNumber);
      if (!day) throw new Error(`Day ${dayNumber} not found in ${destination}`);
      throw new Error(`Session ${session} not found in Day ${dayNumber}`);
    }

    const activityIdx = this.repo.findActivityIndex(activities, activityIdOrTitle);
    if (activityIdx === -1) {
      throw new Error(`Activity not found: "${activityIdOrTitle}" in Day ${dayNumber} ${session}`);
    }

    const activity = activities[activityIdx];
    const wasUpgraded = typeof activity === 'string';

    // Build updates
    const updates: Record<string, unknown> = { booking_status: status };
    if (ref !== undefined) updates.booking_ref = ref;
    if (bookBy !== undefined) updates.book_by = bookBy;
    if (status === 'booked' || status === 'pending' || status === 'waitlist') {
      updates.booking_required = true;
    }
    if (wasUpgraded) {
      updates.booking_required = true;
    }

    const previousStatus = typeof activity === 'string'
      ? undefined
      : (activity as Record<string, unknown>).booking_status as string | undefined;

    const activityObj = this.repo.updateActivityAtIndex(
      destination, dayNumber, session, activityIdx, updates
    );
    this.repo.touchItinerary(destination, this.timestamp);

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

  findActivity(
    destination: string,
    idOrTitle: string
  ): { dayNumber: number; session: SessionType; activity: string | Record<string, unknown>; isString: boolean } | null {
    return this.repo.findActivity(destination, idOrTitle);
  }

  /**
   * Get a specific day from itinerary.
   * Public for backward compat (tests, cascade runner).
   */
  getDay(destination: string, dayNumber: number): Record<string, unknown> | null {
    return this.repo.getDay(destination, dayNumber);
  }

  setDayTheme(destination: string, dayNumber: number, theme: string | null): void {
    this.repo.setDayField(destination, dayNumber, 'theme', theme);
    this.repo.touchItinerary(destination, this.timestamp);

    this.emitEvent({
      event: 'itinerary_day_theme_set',
      destination,
      process: 'process_5_daily_itinerary',
      data: { day_number: dayNumber, theme },
    });
  }

  setDayWeather(destination: string, dayNumber: number, weather: DayWeather): void {
    this.repo.setDayField(destination, dayNumber, 'weather', weather);
    this.repo.touchItinerary(destination, this.timestamp);

    this.emitEvent({
      event: 'weather_updated',
      destination,
      process: 'process_5_daily_itinerary',
      data: { day_number: dayNumber, source_id: weather.source_id },
    });
  }

  setSessionFocus(
    destination: string,
    dayNumber: number,
    session: SessionType,
    focus: string | null
  ): void {
    this.repo.setSessionField(destination, dayNumber, session, 'focus', focus);
    this.repo.touchItinerary(destination, this.timestamp);

    this.emitEvent({
      event: 'itinerary_session_focus_set',
      destination,
      process: 'process_5_daily_itinerary',
      data: { day_number: dayNumber, session, focus },
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
    this.repo.setProcessStatusData(destination, process, newStatus, this.timestamp);

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

  getActiveDestination(): string {
    return this.repo.getActiveDestination();
  }

  setActiveDestination(destination: string): void {
    const previous = this.repo.getActiveDestination();
    if (previous !== destination) {
      this.repo.setActiveDestination(destination);
      this.repo.setEventLogActiveDestination(destination);
      this.emitEvent({
        event: 'active_destination_changed',
        data: { from: previous, to: destination },
      });
    }
  }

  setFocus(destination: string, process: ProcessId): void {
    const previous = this.repo.getEventLog().current_focus;
    const newFocus = `${destination}.${process}`;
    this.repo.setEventLogFocus(newFocus);
    this.emitEvent({
      event: 'focus_changed',
      destination,
      process,
      data: { from: previous, to: newFocus },
    });
  }

  setNextActions(actions: string[]): void {
    const previous = this.repo.getNextActions();
    this.repo.setNextActions(actions);
    this.emitEvent({
      event: 'next_actions_updated',
      data: { from: previous, to: actions },
    });
  }

  getNextActions(): string[] {
    return this.repo.getNextActions();
  }

  // ============================================================================
  // File I/O (legacy — for non-DB constructor paths)
  // ============================================================================

  private loadPlanFromFile(filePath?: string): TravelPlanMinimal {
    const p = filePath || this.planPath;
    if (!existsSync(p)) {
      throw new Error(`Travel plan not found: ${p}`);
    }
    const content = readFileSync(p, 'utf-8');
    const parsed = JSON.parse(content);
    return validateTravelPlan(parsed) as TravelPlanMinimal;
  }

  private loadEventLogFromFile(plan?: TravelPlanMinimal, filePath?: string): EventLogState {
    const p = filePath || this.statePath;
    if (!existsSync(p)) {
      return {
        session: new Date().toISOString().split('T')[0],
        project: DEFAULTS.project,
        version: '3.0',
        active_destination: plan?.active_destination || '',
        current_focus: '',
        event_log: [],
        global_processes: {},
        destinations: {},
      };
    }
    const content = readFileSync(p, 'utf-8');
    const parsed = JSON.parse(content);
    return validateEventLogState(parsed) as EventLogState;
  }

  // ============================================================================
  // Persistence
  // ============================================================================

  async save(): Promise<void> {
    if (this.skipSave) return;
    await this.repo.save(this.planId, this.repo.getSchemaVersion());
  }

  /**
   * Save with operation tracking: wraps save() with run audit trail.
   * Records run start/complete/fail in operation_runs table.
   * Version is bumped atomically inside the normalized table transaction.
   *
   * @param commandType - CLI command name (e.g., 'set-dates', 'mark-booked')
   * @param commandSummary - Human-readable summary (e.g., '2026-02-13 2026-02-17')
   * @returns run_id and new version number
   */
  async saveWithTracking(
    commandType: string,
    commandSummary?: string
  ): Promise<{ run_id: string; version: number }> {
    if (this.skipSave) return { run_id: 'skip', version: 0 };

    const runId = crypto.randomUUID();
    const versionBefore = this.repo.getVersion();

    // Log run start (awaited — ensures row exists before logRunComplete can UPDATE it)
    await this.logRunStart(runId, commandType, commandSummary, versionBefore).catch(() => {});

    try {
      await this.repo.save(this.planId, this.repo.getSchemaVersion());
      const versionAfter = this.repo.getVersion();

      // Log run complete (fire-and-forget, warn on failure)
      this.logRunComplete(runId, versionAfter).catch((e) => {
        console.error(`[op-tracking] failed to log run completion: ${(e as Error).message}`);
      });

      return { run_id: runId, version: versionAfter };
    } catch (err) {
      // Log run failure (fire-and-forget, warn on failure)
      this.logRunFailed(runId, err).catch((e) => {
        console.error(`[op-tracking] failed to log run failure: ${(e as Error).message}`);
      });
      throw err;
    }
  }

  // ============================================================================
  // Operation Run Logging (fire-and-forget helpers)
  // ============================================================================

  private async logRunStart(
    runId: string,
    commandType: string,
    summary: string | undefined,
    version: number
  ): Promise<void> {
    const pathMod = require('node:path');
    const { TursoPipelineClient } = require(
      pathMod.resolve(__dirname, '..', '..', 'scripts', 'turso-pipeline')
    );
    const client = new TursoPipelineClient();
    await client.execute(
      `INSERT INTO operation_runs (run_id, plan_id, command_type, command_summary, status, version_before, started_at)
       VALUES (${sqlTextFn(runId)}, ${sqlTextFn(this.planId)}, ${sqlTextFn(commandType)}, ${sqlTextFn(summary ?? null)}, 'started', ${version}, datetime('now'))`
    );
  }

  private async logRunComplete(runId: string, versionAfter: number): Promise<void> {
    const pathMod = require('node:path');
    const { TursoPipelineClient } = require(
      pathMod.resolve(__dirname, '..', '..', 'scripts', 'turso-pipeline')
    );
    const client = new TursoPipelineClient();
    await client.execute(
      `UPDATE operation_runs SET status = 'completed', version_after = ${versionAfter}, completed_at = datetime('now') WHERE run_id = ${sqlTextFn(runId)}`
    );
  }

  private async logRunFailed(runId: string, err: unknown): Promise<void> {
    const pathMod = require('node:path');
    const { TursoPipelineClient } = require(
      pathMod.resolve(__dirname, '..', '..', 'scripts', 'turso-pipeline')
    );
    const client = new TursoPipelineClient();
    const msg = err instanceof Error ? err.message.substring(0, 500) : 'unknown';
    await client.execute(
      `UPDATE operation_runs SET status = 'failed', error_message = ${sqlTextFn(msg)}, completed_at = datetime('now') WHERE run_id = ${sqlTextFn(runId)}`
    );
  }

  /** Get the plan ID (for operation tracking). */
  getPlanId(): string {
    return this.planId;
  }

  /**
   * Get current plan (for reading).
   */
  getPlan(): TravelPlanMinimal {
    return this.repo.getPlan();
  }

  /**
   * Load travel plan from file with Zod validation.
   * @deprecated Use StateManager.create() factory instead.
   */
  loadPlan(p?: string): TravelPlanMinimal {
    return this.loadPlanFromFile(p);
  }

  /**
   * Load event log from file with Zod validation.
   * @deprecated Use StateManager.create() factory instead.
   */
  loadEventLog(p?: string): EventLogState {
    return this.loadEventLogFromFile(undefined, p);
  }

  // ============================================================================
  // Normalization (legacy schema migration)
  // ============================================================================

  private normalizePlan(plan: TravelPlanMinimal): void {
    for (const dest of Object.values(plan.destinations)) {
      const p34 = (dest as Record<string, unknown>)['process_3_4_packages'] as Record<string, unknown> | undefined;
      if (!p34 || typeof p34 !== 'object') continue;

      if (!p34['results'] || typeof p34['results'] !== 'object') {
        p34['results'] = {};
      }
      const results = p34['results'] as Record<string, unknown>;

      if (Array.isArray(p34['offers']) && !Array.isArray(results['offers'])) {
        results['offers'] = p34['offers'];
        delete p34['offers'];
      }

      if (p34['chosen_offer'] && results['chosen_offer'] == null) {
        const maybeMeta = p34['chosen_offer'] as Record<string, unknown>;
        if (typeof maybeMeta?.id === 'string' && typeof maybeMeta?.selected_date === 'string') {
          // metadata — keep
        } else {
          results['chosen_offer'] = p34['chosen_offer'];
        }
      }

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
