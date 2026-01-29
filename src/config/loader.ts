/**
 * Configuration Loader
 *
 * Loads destinations and OTA sources configuration.
 * Provides discovery APIs for skill composition.
 */

import * as fs from 'fs';
import * as path from 'path';

export interface DestinationConfig {
  slug: string;
  display_name: string;
  ref_id: string;
  ref_path: string;
  timezone: string;
  currency: string;
  markets: string[];
  primary_airports: string[];
  language: string;
}

export interface OtaSourceConfig {
  source_id: string;
  display_name: string;
  display_name_en: string;
  types: ('package' | 'flight' | 'hotel')[];
  base_url: string;
  markets: string[];
  currency: string;
  supported: boolean;
  scraper_script: string | null;
  rate_limit?: { requests_per_minute: number };
  promo_codes?: {
    code: string;
    discount: number;
    currency: string;
    min_purchase: number;
    valid_days?: string[];
    expires: string | null;
  }[];
  notes?: string;
}

interface DestinationsFile {
  version: string;
  destinations: Record<string, DestinationConfig>;
  default_destination: string;
}

interface OtaSourcesFile {
  version: string;
  sources: Record<string, OtaSourceConfig>;
}

// Cached configs
let destinationsCache: DestinationsFile | null = null;
let otaSourcesCache: OtaSourcesFile | null = null;

/**
 * Get the project root directory.
 */
function getProjectRoot(): string {
  // Walk up from current file to find package.json
  let dir = __dirname;
  while (dir !== path.dirname(dir)) {
    if (fs.existsSync(path.join(dir, 'package.json'))) {
      return dir;
    }
    dir = path.dirname(dir);
  }
  // Fallback: assume we're in src/config
  return path.resolve(__dirname, '../..');
}

/**
 * Load destinations configuration.
 */
export function loadDestinations(): DestinationsFile {
  if (destinationsCache) return destinationsCache;

  const configPath = path.join(getProjectRoot(), 'data/destinations.json');
  if (!fs.existsSync(configPath)) {
    throw new Error(`Destinations config not found: ${configPath}`);
  }

  const content = fs.readFileSync(configPath, 'utf-8');
  destinationsCache = JSON.parse(content) as DestinationsFile;
  return destinationsCache;
}

/**
 * Load OTA sources configuration.
 */
export function loadOtaSources(): OtaSourcesFile {
  if (otaSourcesCache) return otaSourcesCache;

  const configPath = path.join(getProjectRoot(), 'data/ota-sources.json');
  if (!fs.existsSync(configPath)) {
    throw new Error(`OTA sources config not found: ${configPath}`);
  }

  const content = fs.readFileSync(configPath, 'utf-8');
  otaSourcesCache = JSON.parse(content) as OtaSourcesFile;
  return otaSourcesCache;
}

/**
 * Clear cached configurations (for testing).
 */
export function clearConfigCache(): void {
  destinationsCache = null;
  otaSourcesCache = null;
}

// ============ Destination Discovery APIs ============

/**
 * Get all available destination slugs.
 */
export function getAvailableDestinations(): string[] {
  const config = loadDestinations();
  return Object.keys(config.destinations);
}

/**
 * Get destination configuration by slug.
 */
export function getDestinationConfig(slug: string): DestinationConfig | null {
  const config = loadDestinations();
  return config.destinations[slug] || null;
}

/**
 * Resolve destination reference file path.
 * Returns absolute path to the destination's JSON reference file.
 */
export function resolveDestinationRefPath(slug: string): string | null {
  const destConfig = getDestinationConfig(slug);
  if (!destConfig) {
    // Try to find by ref_id (e.g., "tokyo" matches "tokyo_2026")
    const config = loadDestinations();
    for (const dest of Object.values(config.destinations)) {
      if (slug.toLowerCase().includes(dest.ref_id.toLowerCase())) {
        const refPath = path.join(getProjectRoot(), dest.ref_path);
        return fs.existsSync(refPath) ? refPath : null;
      }
    }
    return null;
  }

  const refPath = path.join(getProjectRoot(), destConfig.ref_path);
  return fs.existsSync(refPath) ? refPath : null;
}

/**
 * Get default destination slug.
 */
export function getDefaultDestination(): string {
  const config = loadDestinations();
  return config.default_destination;
}

/**
 * Get currency for a destination.
 */
export function getDestinationCurrency(slug: string): string {
  const destConfig = getDestinationConfig(slug);
  return destConfig?.currency || 'JPY';
}

// ============ OTA Source Discovery APIs ============

/**
 * Get all available OTA source IDs.
 */
export function getAvailableOtaSources(): string[] {
  const config = loadOtaSources();
  return Object.keys(config.sources);
}

/**
 * Get supported OTA sources (scraper available).
 */
export function getSupportedOtaSources(): string[] {
  const config = loadOtaSources();
  return Object.entries(config.sources)
    .filter(([_, source]) => source.supported)
    .map(([id]) => id);
}

/**
 * Get OTA source configuration by ID.
 */
export function getOtaSourceConfig(sourceId: string): OtaSourceConfig | null {
  const config = loadOtaSources();
  return config.sources[sourceId] || null;
}

/**
 * Get currency for an OTA source.
 */
export function getOtaSourceCurrency(sourceId: string): string {
  const sourceConfig = getOtaSourceConfig(sourceId);
  return sourceConfig?.currency || 'TWD';
}

/**
 * Get OTA sources available for a market (e.g., "TW" for Taiwan).
 */
export function getOtaSourcesForMarket(market: string): OtaSourceConfig[] {
  const config = loadOtaSources();
  return Object.values(config.sources).filter((source) =>
    source.markets.includes(market)
  );
}

/**
 * Get OTA sources that support a specific type.
 */
export function getOtaSourcesByType(
  type: 'package' | 'flight' | 'hotel'
): OtaSourceConfig[] {
  const config = loadOtaSources();
  return Object.values(config.sources).filter((source) =>
    source.types.includes(type)
  );
}
