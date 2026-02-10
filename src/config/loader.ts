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
  coordinates?: { lat: number; lon: number };
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
let projectRootCache: string | null = null;

/**
 * Get the project root directory.
 */
function getProjectRoot(): string {
  if (projectRootCache) return projectRootCache;
  // Walk up from current file to find package.json
  let dir = __dirname;
  while (dir !== path.dirname(dir)) {
    if (fs.existsSync(path.join(dir, 'package.json'))) {
      projectRootCache = dir;
      return dir;
    }
    dir = path.dirname(dir);
  }
  // Fallback: assume we're in src/config
  projectRootCache = path.resolve(__dirname, '../..');
  return projectRootCache;
}

function resolveRepoPath(relPath: string, context: string): string {
  const root = getProjectRoot();
  if (!relPath || typeof relPath !== 'string') {
    throw new Error(`${context}: expected a non-empty path string`);
  }
  if (path.isAbsolute(relPath)) {
    throw new Error(`${context}: path must be repo-relative, got absolute path: ${relPath}`);
  }
  const resolved = path.resolve(root, relPath);
  const relative = path.relative(root, resolved);
  if (relative.startsWith('..') || path.isAbsolute(relative)) {
    throw new Error(`${context}: path escapes project root: ${relPath}`);
  }
  return resolved;
}

/**
 * Load destinations configuration.
 */
export function loadDestinations(): DestinationsFile {
  if (destinationsCache) return destinationsCache;

  const configPath = resolveRepoPath('data/destinations.json', 'Destinations config');
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

  const configPath = resolveRepoPath('data/ota-sources.json', 'OTA sources config');
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
  projectRootCache = null;
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
        const refPath = resolveRepoPath(dest.ref_path, `Destination ref_path (${dest.slug})`);
        return fs.existsSync(refPath) ? refPath : null;
      }
    }
    return null;
  }

  const refPath = resolveRepoPath(destConfig.ref_path, `Destination ref_path (${destConfig.slug})`);
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
    .filter(([_, source]) => {
      if (!source.supported) return false;
      if (!source.scraper_script) return false;
      const scriptPath = resolveRepoPath(source.scraper_script, `OTA scraper_script (${source.source_id})`);
      return fs.existsSync(scriptPath);
    })
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
  if (!sourceConfig) {
    const available = getAvailableOtaSources();
    throw new Error(`Unknown OTA source: ${sourceId}. Available: ${available.join(', ')}`);
  }
  return sourceConfig.currency;
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
