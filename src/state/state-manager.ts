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
