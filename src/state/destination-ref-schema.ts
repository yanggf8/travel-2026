/**
 * Destination Reference Schema
 *
 * Zod validation for destination reference files (e.g., tokyo.json).
 * Used by populate-itinerary and other skills that consume POI/cluster data.
 *
 * Reference files live at: src/skills/travel-shared/references/destinations/{id}.json
 */

import { z } from 'zod';

// ============================================================================
// Schema Version
// ============================================================================

export const DESTINATION_REF_VERSION = '1.0.0';

// ============================================================================
// Area Schema
// ============================================================================

export const AreaSchema = z.object({
  id: z.string(),
  name: z.string(),
  type: z.string(),  // commercial, traditional, entertainment, luxury, etc.
  stations: z.array(z.string()),
  vibe: z.string().optional(),
  best_for: z.array(z.string()).optional(),
}).passthrough();

// ============================================================================
// POI Schema
// ============================================================================

export const POISchema = z.object({
  id: z.string(),
  title: z.string(),
  area: z.string(),  // References areas[].id
  nearest_station: z.string().nullable().optional(),
  duration_min: z.number().nullable().optional(),
  booking_required: z.boolean().optional(),
  booking_url: z.string().nullable().optional(),
  cost_estimate: z.number().nullable().optional(),
  tags: z.array(z.string()).optional(),
  notes: z.string().nullable().optional(),
  hours: z.string().nullable().optional(),
  address: z.string().nullable().optional(),
}).passthrough();

// ============================================================================
// Cluster Schema
// ============================================================================

export const ClusterSchema = z.object({
  name: z.string(),
  description: z.string().optional(),
  pois: z.array(z.string()),  // References pois[].id
  duration_min: z.number().optional(),
  best_area: z.string().optional(),  // References areas[].id
}).passthrough();

// ============================================================================
// Transit Estimate Schema
// ============================================================================

export const TransitEstimateSchema = z.object({
  minutes: z.number(),
  line: z.string(),
}).passthrough();

// ============================================================================
// Root Destination Reference Schema
// ============================================================================

export const DestinationRefSchema = z.object({
  destination_id: z.string(),
  display_name: z.string(),
  country: z.string(),
  timezone: z.string().optional(),
  currency: z.string().optional(),
  primary_airports: z.array(z.string()).optional(),

  areas: z.array(AreaSchema),
  pois: z.array(POISchema),
  clusters: z.record(z.string(), ClusterSchema),

  transit_estimates: z.record(z.string(), TransitEstimateSchema).optional(),
  tips: z.array(z.string()).optional(),
}).passthrough();

// ============================================================================
// Inferred Types
// ============================================================================

export type DestinationRef = z.infer<typeof DestinationRefSchema>;
export type Area = z.infer<typeof AreaSchema>;
export type POI = z.infer<typeof POISchema>;
export type Cluster = z.infer<typeof ClusterSchema>;
export type TransitEstimate = z.infer<typeof TransitEstimateSchema>;

// ============================================================================
// Validation Helpers
// ============================================================================

/**
 * Validate destination reference with detailed error messages.
 */
export function validateDestinationRef(data: unknown, refPath?: string): DestinationRef {
  const result = DestinationRefSchema.safeParse(data);
  if (!result.success) {
    const formatted = result.error.issues.map((e: z.ZodIssue) =>
      `  - ${e.path.join('.')}: ${e.message}`
    ).join('\n');
    const hint = refPath ? `\n\nFile: ${refPath}` : '';
    throw new Error(
      `Destination reference validation failed:\n${formatted}${hint}\n\n` +
      `Schema version: ${DESTINATION_REF_VERSION}`
    );
  }
  return result.data;
}

/**
 * Safe parse (returns success/error instead of throwing).
 */
export function safeParseDestinationRef(data: unknown) {
  return DestinationRefSchema.safeParse(data);
}

/**
 * Validate internal consistency of a destination reference.
 * Checks that:
 * - All POI area references exist in areas[]
 * - All cluster POI references exist in pois[]
 * - All cluster best_area references exist in areas[]
 */
export function validateDestinationRefConsistency(ref: DestinationRef, refPath?: string): string[] {
  const warnings: string[] = [];
  const areaIds = new Set(ref.areas.map(a => a.id));
  const poiIds = new Set(ref.pois.map(p => p.id));

  // Check POI area references
  for (const poi of ref.pois) {
    if (poi.area && !areaIds.has(poi.area)) {
      warnings.push(`POI "${poi.id}" references unknown area "${poi.area}"`);
    }
  }

  // Check cluster POI references
  for (const [clusterId, cluster] of Object.entries(ref.clusters)) {
    for (const poiId of cluster.pois) {
      if (!poiIds.has(poiId)) {
        warnings.push(`Cluster "${clusterId}" references unknown POI "${poiId}"`);
      }
    }
    if (cluster.best_area && !areaIds.has(cluster.best_area)) {
      warnings.push(`Cluster "${clusterId}" references unknown area "${cluster.best_area}"`);
    }
  }

  return warnings;
}
