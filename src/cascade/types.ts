/**
 * Cascade Runner Types
 * Schema compatibility: travel-plan.json ^4.2.0
 */

// ============================================================================
// Cascade Rule Types
// ============================================================================

export interface CascadeTrigger {
  id: string;
  trigger: string;
  reset?: string[];
  action?: 'populate';
  populate_map?: Record<string, string>;
  set_source?: string;
  condition?: {
    field: string;
    changed: boolean;
  };
  scope: 'all_destinations' | 'current_destination' | 'new_active_destination';
}

export interface CascadeRules {
  wildcard_expansion: {
    mode: 'schema_driven';
    note: string;
    examples: Record<string, string[]>;
  };
  triggers: CascadeTrigger[];
}

// ============================================================================
// Cascade State Types
// ============================================================================

export interface ProcessState {
  dirty: boolean;
  last_changed: string | null;
}

export interface CascadeState {
  last_cascade_run: string;
  global: {
    process_1_date_anchor: ProcessState;
    /**
     * Optional marker to detect `active_destination_change` deterministically.
     * If missing, the runner will not fire `active_destination_change`.
     */
    active_destination_last?: string;
  };
  destinations: Record<string, Record<string, ProcessState>>;
}

// ============================================================================
// Plan Types (what the runner computes)
// ============================================================================

export interface ResetAction {
  type: 'reset';
  destination: string;
  process: string;
  reason: string;
  triggered_by: string;
}

export interface PopulateAction {
  type: 'populate';
  destination: string;
  source_path: string;
  target_path: string;
  set_source: string;
  triggered_by: string;
}

export interface DirtyFlagUpdate {
  type: 'dirty_flag';
  destination: string;
  process: string;
  dirty: boolean;
  last_changed: string | null;
}

export interface GlobalDirtyFlagUpdate {
  type: 'global_dirty_flag';
  process: string;
  dirty: boolean;
  last_changed: string | null;
}

export type CascadeAction = ResetAction | PopulateAction | DirtyFlagUpdate | GlobalDirtyFlagUpdate;

export interface CascadePlan {
  computed_at: string;
  triggers_evaluated: string[];
  actions: CascadeAction[];
  warnings: string[];
}

// ============================================================================
// Execution Result
// ============================================================================

export interface CascadeResult {
  success: boolean;
  plan: CascadePlan;
  applied: boolean;
  output_path: string | null;
  errors: string[];
}

// ============================================================================
// CLI Options
// ============================================================================

export interface CascadeCliOptions {
  input: string;
  output?: string;
  apply: boolean;
  format: 'text' | 'json';
  verbose: boolean;
}

// ============================================================================
// Schema Contract (from travel-plan.json)
// ============================================================================

export interface SchemaContract {
  id_convention: string;
  slug_is_authoritative: boolean;
  display_names_in: string;
  timestamps: string;
  currency: string;
  process_nodes: string[];
}

// ============================================================================
// Minimal Travel Plan Types (what cascade runner needs)
// ============================================================================

export interface TravelPlanMinimal {
  schema_version: string;
  schema_contract: SchemaContract;
  active_destination: string;
  cascade_rules: CascadeRules;
  cascade_state: CascadeState;
  destinations: Record<string, {
    slug: string;
    status: 'active' | 'archived';
    process_3_4_packages?: {
      selected_offer_id: string | null;
      results?: {
        chosen_offer: unknown | null;
      };
    };
    [key: string]: unknown;
  }>;
  [key: string]: unknown;
}
