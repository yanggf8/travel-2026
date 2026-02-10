/**
 * Zod Schemas for Travel Plan Validation
 *
 * Runtime validation for travel-plan.json and state.json.
 * Validates on load and save to catch corruption early.
 */

import { z } from 'zod';

// ============================================================================
// Schema Version (update when schema changes)
// ============================================================================

export const SCHEMA_VERSION = '4.2.0';

// ============================================================================
// Enums and Constants
// ============================================================================

export const ProcessStatusSchema = z.enum([
  'pending',
  'researching',
  'researched',
  'selecting',
  'selected',
  'populated',
  'booking',
  'booked',
  'confirmed',
  'skipped',
]);

export const ProcessIdSchema = z.enum([
  'process_1_date_anchor',
  'process_2_destination',
  'process_3_4_packages',
  'process_3_transportation',
  'process_4_accommodation',
  'process_5_daily_itinerary',
]);

export const DestinationStatusSchema = z.enum(['active', 'archived']);

export const DayTypeSchema = z.enum(['arrival', 'full', 'departure']);

export const SessionTypeSchema = z.enum(['morning', 'afternoon', 'evening']);

export const PrioritySchema = z.enum(['must', 'want', 'optional']);

export const BookingStatusSchema = z.enum(['not_required', 'pending', 'booked', 'waitlist']);

export const AvailabilitySchema = z.enum(['available', 'sold_out', 'limited', 'unknown']);

export const TimeHHMMSchema = z.string().regex(/^\d{2}:\d{2}$/, 'Must be HH:MM');

// Ground transport (airport transfers)
export const TransferStatusSchema = z.enum(['planned', 'booked']);

export const TransportOptionSchema = z.object({
  id: z.string(),
  title: z.string(),
  route: z.string(),
  duration_min: z.number().nullable().optional(),
  price_yen: z.number().nullable().optional(),
  schedule: z.string().nullable().optional(),
  booking_url: z.string().nullable().optional(),
  notes: z.string().nullable().optional(),
  tags: z.array(z.string()).optional(),
}).passthrough();

export const TransportSegmentSchema = z.object({
  status: TransferStatusSchema,
  selected: TransportOptionSchema.nullable().optional(),
  candidates: z.array(TransportOptionSchema),
}).passthrough();

export const AirportTransfersSchema = z.object({
  arrival: TransportSegmentSchema.optional(),
  departure: TransportSegmentSchema.optional(),
}).passthrough();

// ============================================================================
// Dirty Flags & Cascade State
// ============================================================================

export const DirtyFlagSchema = z.object({
  dirty: z.boolean(),
  last_changed: z.string().nullable(),
});

export const GlobalDirtyFlagsSchema = z.object({
  process_1_date_anchor: DirtyFlagSchema,
  active_destination_last: z.string().optional(),
}).passthrough();

export const CascadeStateSchema = z.object({
  last_cascade_run: z.string(),
  global: GlobalDirtyFlagsSchema,
  destinations: z.record(z.string(), z.record(z.string(), DirtyFlagSchema)),
});

// ============================================================================
// Flight & Hotel (Package Components)
// ============================================================================

export const FlightLegSchema = z.object({
  flight_number: z.string().nullable().optional(),
  departure_airport_code: z.string(),
  arrival_airport_code: z.string(),
  departure_time: z.string().nullable().optional(),
  arrival_time: z.string().nullable().optional(),
  date: z.string().optional(),
}).passthrough();

export const FlightSchema = z.object({
  airline: z.string(),
  airline_code: z.string().nullable().optional(),
  outbound: FlightLegSchema,
  return: FlightLegSchema.nullable().optional(),
  booked_date: z.string().optional(),
  populated_at: z.string().optional(),
}).passthrough();

export const HotelSchema = z.object({
  name: z.string(),
  slug: z.string().optional(),
  area: z.string(),
  area_type: z.enum(['central', 'airport', 'suburb', 'unknown']).optional(),
  star_rating: z.number().nullable().optional(),
  access: z.array(z.string()).optional(),
  check_in: z.string().optional(),
  populated_at: z.string().optional(),
}).passthrough();

// ============================================================================
// Package Offer
// ============================================================================

export const DatePricingEntrySchema = z.object({
  price: z.number(),
  availability: AvailabilitySchema,
  seats_remaining: z.number().nullable().optional(),
  note: z.string().optional(),
}).passthrough();

// Package subtype for FIT vs group tour distinction
export const PackageSubtypeSchema = z.enum(['fit', 'group', 'semi_fit', 'unknown']);

export const OfferSchema = z.object({
  id: z.string(),
  source_id: z.string(),
  product_code: z.string().optional(),
  url: z.string().optional(),
  scraped_at: z.string().optional(),
  type: z.enum(['package', 'flight', 'hotel', 'activity']),
  // FIT vs Group distinction (for packages)
  package_subtype: PackageSubtypeSchema.optional(),
  guided: z.boolean().optional(), // Has tour guide/leader
  meals_included: z.number().optional(), // Number of meals included
  duration_days: z.number().optional(),
  currency: z.string(),
  price_per_person: z.number(),
  price_total: z.number().optional(),
  availability: AvailabilitySchema,
  seats_remaining: z.number().nullable().optional(),
  // Baggage
  baggage_included: z.boolean().nullable().optional(),
  baggage_kg: z.number().nullable().optional(),
  flight: FlightSchema.optional(),
  hotel: HotelSchema.optional(),
  includes: z.array(z.string()).optional(),
  date_pricing: z.record(z.string(), DatePricingEntrySchema).nullable().optional(),
  best_value: z.object({
    date: z.string(),
    price_per_person: z.number(),
    price_total: z.number(),
  }).optional(),
  pros: z.array(z.string()).optional(),
  cons: z.array(z.string()).optional(),
  note: z.string().optional(),
  last_price_check: z.string().optional(),
}).passthrough();

// ============================================================================
// Itinerary (P5)
// ============================================================================

export const ActivitySchema = z.object({
  id: z.string().optional(),
  title: z.string().optional(),
  area: z.string().optional(),
  nearest_station: z.string().nullable().optional(),
  duration_min: z.number().nullable().optional(),
  booking_required: z.boolean().optional(),
  booking_url: z.string().nullable().optional(),
  booking_status: BookingStatusSchema.optional(),
  booking_ref: z.string().optional(),
  book_by: z.string().optional(),  // ISO date: YYYY-MM-DD deadline
  start_time: TimeHHMMSchema.optional(),
  end_time: TimeHHMMSchema.optional(),
  is_fixed_time: z.boolean().optional(),
  cost_estimate: z.number().nullable().optional(),
  tags: z.array(z.string()).optional(),
  notes: z.string().nullable().optional(),
  priority: PrioritySchema.optional(),
}).passthrough();

export const DaySessionSchema = z.object({
  focus: z.string().nullable().optional(),
  activities: z.array(z.union([z.string(), ActivitySchema])).optional(),
  meals: z.array(z.string()).optional(),
  transit_notes: z.string().nullable().optional(),
  booking_notes: z.string().nullable().optional(),
  time_range: z.object({
    start: TimeHHMMSchema,
    end: TimeHHMMSchema,
  }).optional(),
}).passthrough();

export const DayWeatherSchema = z.object({
  temp_high_c: z.number(),
  temp_low_c: z.number(),
  precipitation_pct: z.number().min(0).max(100),
  weather_code: z.number(),
  weather_label: z.string(),
  source_id: z.string(),
  sourced_at: z.string(),
});

export const ItineraryDaySchema = z.object({
  date: z.string(),
  day_number: z.number(),
  day_type: DayTypeSchema,
  status: z.enum(['draft', 'planned', 'confirmed']).optional(),
  theme: z.string().nullable().optional(),
  weather: DayWeatherSchema.optional(),
  morning: DaySessionSchema.optional(),
  afternoon: DaySessionSchema.optional(),
  evening: DaySessionSchema.optional(),
}).passthrough();

// ============================================================================
// Process Nodes
// ============================================================================

export const ProcessNodeBaseSchema = z.object({
  status: ProcessStatusSchema,
  updated_at: z.string().optional(),
}).passthrough();

export const DateAnchorSchema = ProcessNodeBaseSchema.extend({
  confirmed_dates: z.object({
    start: z.string(),
    end: z.string(),
  }).optional(),
  days: z.number().optional(),
}).passthrough();

export const DestinationProcessSchema = ProcessNodeBaseSchema.extend({
  origin_city: z.string().optional(),
  origin_country: z.string().optional(),
  destination_country: z.string().optional(),
  region: z.string().optional(),
  primary_airport: z.string().optional(),
  cities: z.array(z.object({
    slug: z.string(),
    display_name: z.string(),
    role: z.string(),
    nights: z.number(),
    attractions: z.array(z.string()).optional(),
  }).passthrough()).optional(),
}).passthrough();

export const PackagesProcessSchema = ProcessNodeBaseSchema.extend({
  selected_offer_id: z.string().nullable().optional(),
  results: z.object({
    offers: z.array(OfferSchema).optional(),
    chosen_offer: OfferSchema.optional(),
    provenance: z.array(z.object({
      source_id: z.string(),
      scraped_at: z.string(),
      offers_found: z.number(),
      note: z.string().optional(),
    }).passthrough()).optional(),
    warnings: z.array(z.string()).optional(),
  }).passthrough().optional(),
  chosen_offer: z.object({
    id: z.string(),
    selected_date: z.string(),
    selected_at: z.string(),
  }).optional(),
}).passthrough();

// Flight can be either direct booking info OR research results with candidates
export const FlightDataSchema = z.union([
  // Direct flight info (booked/populated)
  FlightSchema,
  // Research results with candidates
  z.object({
    status: z.string(),
    candidates: z.array(z.unknown()),
  }).passthrough(),
]);

export const TransportationProcessSchema = ProcessNodeBaseSchema.extend({
  source: z.string().nullable().optional(),
  flight: FlightDataSchema.optional(),
  home_to_airport: z.object({
    status: z.string(),
    candidates: z.array(z.unknown()),
  }).optional(),
  airport_to_hotel: z.object({
    status: z.string(),
    candidates: z.array(z.unknown()),
  }).optional(),
  airport_transfers: AirportTransfersSchema.optional(),
  populated_from: z.string().optional(),
}).passthrough();

// Hotel can be either direct booking info OR research results with candidates
export const HotelDataSchema = z.union([
  // Direct hotel info (booked/populated)
  HotelSchema,
  // Research results with candidates
  z.object({
    status: z.string(),
    candidates: z.array(z.unknown()),
  }).passthrough(),
]);

export const AccommodationProcessSchema = ProcessNodeBaseSchema.extend({
  source: z.string().nullable().optional(),
  location_zone: z.object({
    status: z.string().optional(),
    selected_area: z.string().nullable().optional(),
    candidates: z.array(z.object({
      slug: z.string(),
      display_name: z.string(),
      pros: z.array(z.string()).optional(),
      cons: z.array(z.string()).optional(),
    }).passthrough()).optional(),
  }).optional(),
  hotel: HotelDataSchema.optional(),
  populated_from: z.string().optional(),
}).passthrough();

export const ItineraryProcessSchema = ProcessNodeBaseSchema.extend({
  days: z.array(ItineraryDaySchema).optional(),
  scaffolded_at: z.string().optional(),
  populated_at: z.string().optional(),
  notes: z.string().optional(),
}).passthrough();

// ============================================================================
// Destination Section Schema Map
// ============================================================================

/**
 * Map of destination-level section IDs to their Zod schemas.
 * Used for per-section validation without validating the entire plan.
 */
export const DestinationSectionSchemas = {
  process_1_date_anchor: DateAnchorSchema,
  process_2_destination: DestinationProcessSchema,
  process_3_4_packages: PackagesProcessSchema,
  process_3_transportation: TransportationProcessSchema,
  process_4_accommodation: AccommodationProcessSchema,
  process_5_daily_itinerary: ItineraryProcessSchema,
} as const;

export type DestinationSectionId = keyof typeof DestinationSectionSchemas;

export const DESTINATION_SECTION_IDS: DestinationSectionId[] = [
  'process_1_date_anchor',
  'process_2_destination',
  'process_3_4_packages',
  'process_3_transportation',
  'process_4_accommodation',
  'process_5_daily_itinerary',
];

// ============================================================================
// Section Validation
// ============================================================================

export interface SectionValidationError {
  path: string;
  message: string;
}

export interface SectionValidationResult {
  valid: boolean;
  sectionId: DestinationSectionId;
  present: boolean;
  errors?: SectionValidationError[];
}

export interface DestinationValidationResult {
  valid: boolean;
  destinationSlug: string;
  sections: Map<DestinationSectionId, SectionValidationResult>;
  /** Sections that failed validation */
  invalidSections: DestinationSectionId[];
  /** Sections that are missing (not present in destination) */
  missingSections: DestinationSectionId[];
}

/**
 * Validate a single section of a destination.
 * 
 * @param sectionId - The section to validate (e.g., 'process_5_daily_itinerary')
 * @param data - The section data to validate
 * @returns Validation result with errors if invalid
 */
export function validateDestinationSection(
  sectionId: DestinationSectionId,
  data: unknown
): SectionValidationResult {
  if (data === undefined || data === null) {
    return {
      valid: true, // Missing sections are valid by default (optional)
      sectionId,
      present: false,
    };
  }

  const schema = DestinationSectionSchemas[sectionId];
  const result = schema.safeParse(data);

  if (result.success) {
    return { valid: true, sectionId, present: true };
  }

  return {
    valid: false,
    sectionId,
    present: true,
    errors: result.error.issues.map((e: z.ZodIssue) => ({
      path: e.path.join('.'),
      message: e.message,
    })),
  };
}

/**
 * Validate all sections of a destination independently.
 * Returns per-section results so you can identify exactly which sections are valid/invalid.
 * 
 * @param destinationSlug - Slug for error reporting
 * @param destination - The destination object to validate
 * @param opts - Options: requirePresent forces all sections to be present
 * @returns Aggregated validation result with per-section details
 */
export function validateDestinationSections(
  destinationSlug: string,
  destination: Record<string, unknown>,
  opts?: { requirePresent?: boolean }
): DestinationValidationResult {
  const sections = new Map<DestinationSectionId, SectionValidationResult>();
  const invalidSections: DestinationSectionId[] = [];
  const missingSections: DestinationSectionId[] = [];

  for (const sectionId of DESTINATION_SECTION_IDS) {
    const data = destination[sectionId];
    const result = validateDestinationSection(sectionId, data);
    sections.set(sectionId, result);

    if (!result.present) {
      missingSections.push(sectionId);
      // If requirePresent is true, missing is treated as invalid
      if (opts?.requirePresent) {
        invalidSections.push(sectionId);
      }
    } else if (!result.valid) {
      invalidSections.push(sectionId);
    }
  }

  return {
    valid: invalidSections.length === 0,
    destinationSlug,
    sections,
    invalidSections,
    missingSections,
  };
}

/**
 * Format section validation errors for display.
 * Returns a human-readable string describing validation failures.
 */
export function formatSectionValidationErrors(
  result: DestinationValidationResult
): string {
  if (result.valid) {
    return `Destination "${result.destinationSlug}": all sections valid`;
  }

  const lines: string[] = [
    `Destination "${result.destinationSlug}" validation failed:`,
  ];

  for (const sectionId of result.invalidSections) {
    const sectionResult = result.sections.get(sectionId);
    if (!sectionResult) continue;

    if (!sectionResult.present) {
      lines.push(`  - ${sectionId}: MISSING (required)`);
    } else if (sectionResult.errors) {
      lines.push(`  - ${sectionId}:`);
      for (const err of sectionResult.errors) {
        lines.push(`      ${err.path || '(root)'}: ${err.message}`);
      }
    }
  }

  return lines.join('\n');
}

// ============================================================================
// Destination
// ============================================================================

export const DestinationSchema = z.object({
  slug: z.string(),
  display_name: z.string().optional(),
  status: DestinationStatusSchema,
  created_at: z.string().optional(),
  updated_at: z.string().optional(),
  archived_at: z.string().optional(),
  process_1_date_anchor: DateAnchorSchema.optional(),
  process_2_destination: DestinationProcessSchema.optional(),
  process_3_4_packages: PackagesProcessSchema.optional(),
  process_3_transportation: TransportationProcessSchema.optional(),
  process_4_accommodation: AccommodationProcessSchema.optional(),
  process_5_daily_itinerary: ItineraryProcessSchema.optional(),
}).passthrough();

// ============================================================================
// Root Travel Plan
// ============================================================================

export const TravelPlanSchema = z.object({
  schema_version: z.string().regex(/^\d+\.\d+\.\d+$/, 'Must be semver format'),
  project: z.string().optional(),
  default_timezone: z.string().optional(),
  active_destination: z.string(),
  budget: z.object({
    total_cap: z.number().nullable().optional(),
    flight_cap: z.number().nullable().optional(),
    accommodation_cap: z.number().nullable().optional(),
    daily_cap: z.number().nullable().optional(),
    pax: z.number(),
  }).optional(),
  process_1_date_anchor: z.object({
    status: ProcessStatusSchema,
    updated_at: z.string().optional(),
    set_out_date: z.string().optional(),
    duration_days: z.number().optional(),
    return_date: z.string().optional(),
    flexibility: z.object({
      date_flexible: z.boolean(),
      preferred_dates: z.array(z.string()).optional(),
      avoid_dates: z.array(z.string()).optional(),
      reason: z.string().optional(),
    }).optional(),
  }).passthrough().optional(),
  destinations: z.record(z.string(), DestinationSchema),
  cascade_rules: z.object({
    wildcard_expansion: z.unknown().optional(),
    triggers: z.array(z.unknown()).optional(),
  }).passthrough().optional(),
  cascade_state: CascadeStateSchema,
  canonical_offer_schema: z.unknown().optional(),
  process_precedence: z.unknown().optional(),
  ota_sources: z.record(z.string(), z.unknown()).optional(),
  skill_io_contracts: z.unknown().optional(),
  comparison: z.unknown().optional(),
  schema_contract: z.unknown().optional(),
}).passthrough();

// ============================================================================
// Event Log State (state.json)
// ============================================================================

export const TravelEventSchema = z.object({
  event: z.string(),
  at: z.string(),
  from: ProcessStatusSchema.optional(),
  to: ProcessStatusSchema.optional(),
  destination: z.string().optional(),
  process: z.string().optional(),
  data: z.record(z.string(), z.unknown()).optional(),
}).passthrough();

export const EventLogStateSchema = z.object({
  session: z.string(),
  project: z.string(),
  version: z.string(),
  architecture: z.string().optional(),
  active_destination: z.string(),
  current_focus: z.string(),
  next_actions: z.array(z.string()).optional(),
  event_log: z.array(TravelEventSchema),
  global_processes: z.record(z.string(), z.object({
    state: ProcessStatusSchema,
    events: z.array(TravelEventSchema),
  })).optional(),
  destinations: z.record(z.string(), z.object({
    status: DestinationStatusSchema,
    archived_at: z.string().optional(),
    processes: z.record(z.string(), z.object({
      state: ProcessStatusSchema,
      events: z.array(TravelEventSchema),
    })),
  })),
  transitions: z.record(z.string(), z.array(z.string())).optional(),
  destination_transitions: z.record(z.string(), z.array(z.string())).optional(),
  skills_enhancement: z.unknown().optional(),
}).passthrough();

// ============================================================================
// Inferred Types
// ============================================================================

export type TravelPlan = z.infer<typeof TravelPlanSchema>;
export type EventLogState = z.infer<typeof EventLogStateSchema>;
export type Destination = z.infer<typeof DestinationSchema>;
export type Offer = z.infer<typeof OfferSchema>;
export type ItineraryDay = z.infer<typeof ItineraryDaySchema>;
export type Activity = z.infer<typeof ActivitySchema>;
export type DaySession = z.infer<typeof DaySessionSchema>;

// ============================================================================
// Validation Helpers
// ============================================================================

/**
 * Validate travel plan with detailed error messages.
 */
export function validateTravelPlan(data: unknown): TravelPlan {
  const result = TravelPlanSchema.safeParse(data);
  if (!result.success) {
    const formatted = result.error.issues.map((e: z.ZodIssue) =>
      `  - ${e.path.join('.')}: ${e.message}`
    ).join('\n');
    throw new Error(
      `Travel plan validation failed:\n${formatted}\n\n` +
      `Hint: Check data matches schema v${SCHEMA_VERSION}`
    );
  }
  return result.data;
}

/**
 * Validate event log state with detailed error messages.
 */
export function validateEventLogState(data: unknown): EventLogState {
  const result = EventLogStateSchema.safeParse(data);
  if (!result.success) {
    const formatted = result.error.issues.map((e: z.ZodIssue) =>
      `  - ${e.path.join('.')}: ${e.message}`
    ).join('\n');
    throw new Error(
      `Event log validation failed:\n${formatted}`
    );
  }
  return result.data;
}

/**
 * Safe parse (returns success/error instead of throwing).
 */
export function safeParseTravelPlan(data: unknown) {
  return TravelPlanSchema.safeParse(data);
}

export function safeParseEventLogState(data: unknown) {
  return EventLogStateSchema.safeParse(data);
}
