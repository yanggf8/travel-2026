/**
 * Cascade Runner
 *
 * Executes cascade rules against travel-plan.json
 * - Computes a deterministic plan
 * - Supports dry-run (default) and apply modes
 * - Transactional: plan → diff → apply → update state
 */

import { readFileSync, writeFileSync } from 'fs';
import {
  TravelPlanMinimal,
  CascadePlan,
  CascadeResult,
  CascadeAction,
  ResetAction,
  PopulateAction,
  DirtyFlagUpdate,
  GlobalDirtyFlagUpdate,
  CascadeTrigger,
} from './types';
import { expandPatterns } from './wildcard';

// ============================================================================
// Plan Computation
// ============================================================================

/**
 * Compute cascade plan based on dirty flags and triggers.
 */
export function computePlan(plan: TravelPlanMinimal): CascadePlan {
  const now = new Date().toISOString();
  const actions: CascadeAction[] = [];
  const warnings: string[] = [];
  const triggersEvaluated: string[] = [];
  const triggerSourceClears: Array<GlobalDirtyFlagUpdate | DirtyFlagUpdate> = [];

  const { cascade_rules, cascade_state, active_destination, schema_contract } = plan;
  const processNodes = schema_contract.process_nodes;

  // Sort triggers by id for deterministic execution
  const sortedTriggers = [...cascade_rules.triggers].sort((a, b) => a.id.localeCompare(b.id));

  for (const trigger of sortedTriggers) {
    triggersEvaluated.push(trigger.id);

    // Evaluate trigger conditions
    const shouldFire = evaluateTrigger(trigger, plan, cascade_state);
    if (shouldFire.warning) {
      warnings.push(shouldFire.warning);
    }

    if (!shouldFire.fire) {
      continue;
    }

    // Determine affected destinations
    const destinations = getAffectedDestinations(trigger.scope, active_destination, plan);

    // Clear the trigger-source dirty flag(s) so cascades are idempotent.
    const sourceClears = computeTriggerSourceClears(trigger, plan, cascade_state, destinations, now);
    triggerSourceClears.push(...sourceClears);

    // Handle reset actions
    if (trigger.reset && trigger.reset.length > 0) {
      const expandedTargets = expandPatterns(trigger.reset, processNodes);

      for (const dest of destinations) {
        for (const target of expandedTargets) {
          actions.push({
            type: 'reset',
            destination: dest,
            process: target,
            reason: shouldFire.reason,
            triggered_by: trigger.id,
          } as ResetAction);
        }
      }
    }

    // Handle populate actions
    if (trigger.action === 'populate' && trigger.populate_map) {
      for (const dest of destinations) {
        for (const [sourcePath, targetPath] of Object.entries(trigger.populate_map)) {
          actions.push({
            type: 'populate',
            destination: dest,
            source_path: sourcePath,
            target_path: targetPath,
            set_source: trigger.set_source || 'cascade',
            triggered_by: trigger.id,
          } as PopulateAction);
        }
      }
    }
  }

  // Compute dirty flag updates based on actions
  const dirtyFlagUpdates = computeDirtyFlagUpdates(actions, plan, now);
  actions.push(...dirtyFlagUpdates, ...triggerSourceClears);

  // Sort all actions for deterministic output
  actions.sort((a, b) => {
    if (a.type !== b.type) return a.type.localeCompare(b.type);
    if ('destination' in a && 'destination' in b) {
      if (a.destination !== b.destination) return a.destination.localeCompare(b.destination);
    }
    if ('process' in a && 'process' in b) {
      return a.process.localeCompare(b.process);
    }
    return 0;
  });

  return {
    computed_at: now,
    triggers_evaluated: triggersEvaluated,
    actions,
    warnings,
  };
}

/**
 * Evaluate whether a trigger should fire.
 */
function evaluateTrigger(
  trigger: CascadeTrigger,
  plan: TravelPlanMinimal,
  state: TravelPlanMinimal['cascade_state']
): { fire: boolean; reason: string; warning?: string } {
  switch (trigger.trigger) {
    case 'active_destination_change':
      if (!state.global.active_destination_last) {
        return {
          fire: false,
          reason: '',
          warning:
            'cascade_state.global.active_destination_last is missing; active_destination_change cannot be detected',
        };
      }
      if (state.global.active_destination_last !== plan.active_destination) {
        return { fire: true, reason: `Active destination changed (${state.global.active_destination_last} → ${plan.active_destination})` };
      }
      return { fire: false, reason: '' };

    case 'process_1_date_anchor_change':
      if (state.global.process_1_date_anchor.dirty) {
        return { fire: true, reason: 'Date anchor changed' };
      }
      return { fire: false, reason: '' };

    case 'process_2_destination_change':
      {
        const destinations = getAffectedDestinations(trigger.scope, plan.active_destination, plan);
        for (const destSlug of destinations) {
          const destState = state.destinations[destSlug];
          const p2State = destState?.['process_2_destination'];
          if (!p2State?.dirty) continue;

          if (trigger.condition?.field === 'region' && trigger.condition?.changed) {
            // TODO: compare old vs new region; for now treat dirty as changed.
            return { fire: true, reason: `Destination changed (${destSlug}.process_2_destination)` };
          }
          return { fire: true, reason: `Destination changed (${destSlug}.process_2_destination)` };
        }
        return { fire: false, reason: '' };
      }

    case 'process_3_4_packages_selected':
      // Only fire if chosen_offer changed (guard for idempotency)
      const activeDest = plan.destinations[plan.active_destination];
      if (activeDest?.process_3_4_packages?.results?.chosen_offer) {
        const p34State = state.destinations[plan.active_destination]?.['process_3_4_packages'];
        if (p34State?.dirty) {
          return { fire: true, reason: 'Package selected, populate transport + accommodation' };
        }
      }
      return { fire: false, reason: '' };

    default:
      return { fire: false, reason: `Unknown trigger: ${trigger.trigger}` };
  }
}

/**
 * Get list of destinations affected by a cascade scope.
 */
function getAffectedDestinations(
  scope: CascadeTrigger['scope'],
  activeDestination: string,
  plan: TravelPlanMinimal
): string[] {
  switch (scope) {
    case 'all_destinations':
      return Object.keys(plan.destinations).sort();
    case 'current_destination':
      return [activeDestination];
    case 'new_active_destination':
      return [activeDestination];
    default:
      return [activeDestination];
  }
}

/**
 * Compute dirty flag updates after actions are applied.
 */
function computeDirtyFlagUpdates(
  actions: CascadeAction[],
  plan: TravelPlanMinimal,
  now: string
): DirtyFlagUpdate[] {
  const updates: DirtyFlagUpdate[] = [];
  const processedKeys = new Set<string>();

  // For each reset action, clear the dirty flag
  for (const action of actions) {
    if (action.type === 'reset') {
      const key = `${action.destination}.${action.process}`;
      if (!processedKeys.has(key)) {
        processedKeys.add(key);
        updates.push({
          type: 'dirty_flag',
          destination: action.destination,
          process: action.process,
          dirty: false,
          last_changed: now,
        });
      }
    }
  }

  return updates;
}

function computeTriggerSourceClears(
  trigger: CascadeTrigger,
  plan: TravelPlanMinimal,
  state: TravelPlanMinimal['cascade_state'],
  destinations: string[],
  now: string
): Array<GlobalDirtyFlagUpdate | DirtyFlagUpdate> {
  switch (trigger.trigger) {
    case 'process_1_date_anchor_change':
      if (!state.global.process_1_date_anchor.dirty) return [];
      return [
        {
          type: 'global_dirty_flag',
          process: 'process_1_date_anchor',
          dirty: false,
          last_changed: now,
        },
      ];

    case 'process_2_destination_change': {
      const clears: DirtyFlagUpdate[] = [];
      for (const destSlug of destinations) {
        const p2State = state.destinations[destSlug]?.['process_2_destination'];
        if (p2State?.dirty) {
          clears.push({
            type: 'dirty_flag',
            destination: destSlug,
            process: 'process_2_destination',
            dirty: false,
            last_changed: now,
          });
        }
      }
      return clears;
    }

    case 'process_3_4_packages_selected': {
      const destSlug = plan.active_destination;
      const p34State = state.destinations[destSlug]?.['process_3_4_packages'];
      if (!p34State?.dirty) return [];
      return [
        {
          type: 'dirty_flag',
          destination: destSlug,
          process: 'process_3_4_packages',
          dirty: false,
          last_changed: now,
        },
      ];
    }

    case 'active_destination_change':
      // No dirty flag to clear; active_destination_last is updated on apply.
      return [];

    default:
      return [];
  }
}

// ============================================================================
// Plan Application
// ============================================================================

/**
 * Apply cascade plan to travel plan data.
 * Returns a modified copy (does not mutate input).
 */
export function applyPlan(plan: TravelPlanMinimal, cascadePlan: CascadePlan): TravelPlanMinimal {
  // Deep clone
  const result = JSON.parse(JSON.stringify(plan)) as TravelPlanMinimal;
  const timestamp = cascadePlan.computed_at;

  for (const action of cascadePlan.actions) {
    switch (action.type) {
      case 'reset':
        applyResetAction(result, action, timestamp);
        break;
      case 'populate':
        applyPopulateAction(result, action, timestamp);
        break;
      case 'dirty_flag':
        applyDirtyFlagUpdate(result, action);
        break;
      case 'global_dirty_flag':
        applyGlobalDirtyFlagUpdate(result, action);
        break;
    }
  }

  // Update cascade_state.last_cascade_run
  result.cascade_state.last_cascade_run = cascadePlan.computed_at;
  // Track active destination for deterministic change detection.
  result.cascade_state.global.active_destination_last = result.active_destination;

  return result;
}

function applyResetAction(plan: TravelPlanMinimal, action: ResetAction, timestamp: string): void {
  const dest = plan.destinations[action.destination];
  if (!dest) return;

  const process = dest[action.process] as Record<string, unknown> | undefined;
  if (!process) return;

  // Reset status to pending
  process['status'] = 'pending';
  process['updated_at'] = timestamp;

  // Clear candidates/results depending on process type
  if (action.process === 'process_3_4_packages') {
    if (process['results']) {
      (process['results'] as Record<string, unknown>)['offers'] = [];
      (process['results'] as Record<string, unknown>)['chosen_offer'] = null;
    }
    process['selected_offer_id'] = null;
  } else if (action.process === 'process_3_transportation') {
    const flight = process['flight'] as Record<string, unknown> | undefined;
    if (flight) {
      flight['status'] = 'pending';
      flight['candidates'] = [];
      flight['outbound'] = {};
      flight['return'] = {};
    }
    process['source'] = null;
  } else if (action.process === 'process_4_accommodation') {
    const hotel = process['hotel'] as Record<string, unknown> | undefined;
    if (hotel) {
      hotel['status'] = 'pending';
      hotel['candidates'] = [];
      hotel['selected_hotel'] = null;
    }
    process['source'] = null;
  } else if (action.process === 'process_5_daily_itinerary') {
    process['days'] = [];
  }
}

function applyPopulateAction(plan: TravelPlanMinimal, action: PopulateAction, timestamp: string): void {
  const dest = plan.destinations[action.destination];
  if (!dest) return;

  // Get source value
  const sourceValue = getNestedValue(dest, action.source_path);
  if (sourceValue === undefined) return;

  // Set target value
  setNestedValue(dest, action.target_path, sourceValue);

  // Set source marker
  const targetProcess = action.target_path.split('.')[0];
  const process = dest[targetProcess] as Record<string, unknown> | undefined;
  if (process) {
    process['source'] = action.set_source;
    process['status'] = 'selected';
    process['updated_at'] = timestamp;
  }
}

function applyDirtyFlagUpdate(plan: TravelPlanMinimal, action: DirtyFlagUpdate): void {
  const destState = plan.cascade_state.destinations[action.destination];
  if (!destState) {
    plan.cascade_state.destinations[action.destination] = {};
  }

  plan.cascade_state.destinations[action.destination][action.process] = {
    dirty: action.dirty,
    last_changed: action.last_changed,
  };
}

function applyGlobalDirtyFlagUpdate(plan: TravelPlanMinimal, action: GlobalDirtyFlagUpdate): void {
  plan.cascade_state.global[action.process as 'process_1_date_anchor'] = {
    dirty: action.dirty,
    last_changed: action.last_changed,
  };
}

// ============================================================================
// Utility Functions
// ============================================================================

function getNestedValue(obj: Record<string, unknown>, path: string): unknown {
  const parts = path.split('.');
  let current: unknown = obj;

  for (const part of parts) {
    if (current === null || current === undefined) return undefined;
    if (typeof current !== 'object') return undefined;
    current = (current as Record<string, unknown>)[part];
  }

  return current;
}

function setNestedValue(obj: Record<string, unknown>, path: string, value: unknown): void {
  const parts = path.split('.');
  let current = obj;

  for (let i = 0; i < parts.length - 1; i++) {
    const part = parts[i];
    if (!(part in current) || typeof current[part] !== 'object') {
      current[part] = {};
    }
    current = current[part] as Record<string, unknown>;
  }

  current[parts[parts.length - 1]] = value;
}

// ============================================================================
// File I/O
// ============================================================================

/**
 * Load travel plan from file.
 */
export function loadPlan(inputPath: string): TravelPlanMinimal {
  const content = readFileSync(inputPath, 'utf-8');
  return JSON.parse(content) as TravelPlanMinimal;
}

/**
 * Save travel plan to file.
 */
export function savePlan(plan: TravelPlanMinimal, outputPath: string): void {
  const content = JSON.stringify(plan, null, 2);
  writeFileSync(outputPath, content, 'utf-8');
}

// ============================================================================
// Main Runner
// ============================================================================

export interface RunOptions {
  inputPath: string;
  outputPath?: string;
  apply: boolean;
}

/**
 * Run the cascade processor.
 */
export function run(options: RunOptions): CascadeResult {
  const errors: string[] = [];

  // Load plan
  let plan: TravelPlanMinimal;
  try {
    plan = loadPlan(options.inputPath);
  } catch (e) {
    return {
      success: false,
      plan: { computed_at: new Date().toISOString(), triggers_evaluated: [], actions: [], warnings: [] },
      applied: false,
      output_path: null,
      errors: [`Failed to load plan: ${e}`],
    };
  }

  // Validate schema version
  if (!plan.schema_version.startsWith('4.')) {
    errors.push(`Schema version ${plan.schema_version} not supported. Requires ^4.2.0`);
  }

  // Compute plan
  const cascadePlan = computePlan(plan);

  // If dry-run, return plan without applying
  if (!options.apply) {
    return {
      success: errors.length === 0,
      plan: cascadePlan,
      applied: false,
      output_path: null,
      errors,
    };
  }

  // Apply plan
  const updatedPlan = applyPlan(plan, cascadePlan);

  // Write output
  const outputPath = options.outputPath || options.inputPath;
  try {
    savePlan(updatedPlan, outputPath);
  } catch (e) {
    errors.push(`Failed to write plan: ${e}`);
    return {
      success: false,
      plan: cascadePlan,
      applied: false,
      output_path: null,
      errors,
    };
  }

  return {
    success: errors.length === 0,
    plan: cascadePlan,
    applied: true,
    output_path: outputPath,
    errors,
  };
}
