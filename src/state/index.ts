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
  // Booking status
  BookingStatus,
  BOOKING_STATUSES,
  // Ground transport types
  TransferStatus,
  TRANSFER_STATUSES,
  TransportOption,
  TransportSegment,
  AirportTransfers,
} from './types';

// Zod schemas for runtime validation
export {
  SCHEMA_VERSION,
  TravelPlanSchema,
  EventLogStateSchema,
  BookingStatusSchema,
  TransferStatusSchema,
  TransportOptionSchema,
  TransportSegmentSchema,
  AirportTransfersSchema,
  validateTravelPlan,
  validateEventLogState,
  safeParseTravelPlan,
  safeParseEventLogState,
} from './schemas';

// Destination reference schema
export {
  DESTINATION_REF_VERSION,
  DestinationRefSchema,
  AreaSchema,
  POISchema,
  ClusterSchema,
  validateDestinationRef,
  safeParseDestinationRef,
  validateDestinationRefConsistency,
  type DestinationRef,
  type Area,
  type POI,
  type Cluster,
} from './destination-ref-schema';
