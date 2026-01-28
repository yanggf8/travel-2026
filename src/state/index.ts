/**
 * State Management Module
 *
 * Unified state management for travel planning skills.
 */

export { StateManager, getStateManager, resetStateManager } from './state-manager';
export {
  ProcessStatus,
  ProcessId,
  DirtyFlag,
  CascadeState,
  TravelEvent,
  EventLogState,
  TravelPlanMinimal,
  STATUS_TRANSITIONS,
  // Activity types (P5 Itinerary)
  SessionType,
  Activity,
  DaySession,
  ItineraryDay,
  generateActivityId,
} from './types';
