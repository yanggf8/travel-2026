/**
 * Configuration Loader
 *
 * Loads destinations and OTA sources configuration.
 * Provides discovery APIs for skill composition.
 *
 * Destination config is stored in Turso DB (destination_config, origin_config, global_config).
 * Call loadDestinationConfigFromDb() at startup to populate the in-memory caches.
 * All sync APIs check the DB cache first, falling back to the JSON file if present.
 */

import * as fs from 'fs';
import * as path from 'path';
import { URL } from 'url';

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

// ============================================================================
// Module-level caches
// ============================================================================

/** JSON file cache (backward compat fallback) */
let destinationsCache: DestinationsFile | null = null;
let otaSourcesCache: OtaSourcesFile | null = null;
let projectRootCache: string | null = null;

/** DB-populated caches (populated by loadDestinationConfigFromDb) */
let dbDestCache: Record<string, DestinationConfig> | null = null;
let dbOriginCache: Record<string, Record<string, string | string[]>> | null = null;
let dbGlobalCache: Record<string, string> | null = null;
/** Whether a DB load has been attempted (avoids repeated attempts on DB failure) */
let dbLoadAttempted = false;

// ============================================================================
// Pipeline helper — same pattern as turso-repository.ts
// ============================================================================

function requirePipeline(): { TursoPipelineClient: new (opts?: any) => any } {
  const p = require('node:path');
  return require(p.resolve(__dirname, '..', '..', 'scripts', 'turso-pipeline'));
}

// ============================================================================
// DB loader
// ============================================================================

/**
 * Load destination config from Turso DB into module-level caches.
 * Idempotent — safe to call multiple times; only fetches once per process.
 */
export async function loadDestinationConfigFromDb(): Promise<void> {
  if (dbDestCache !== null) return; // already loaded
  if (dbLoadAttempted) return;      // previous attempt failed, do not retry
  dbLoadAttempted = true;

  try {
    const { TursoPipelineClient } = requirePipeline();
    const client = new TursoPipelineClient();

    const batchResponse = await client.executeBatch([
      'SELECT slug, display_name, ref_id, ref_path, timezone, currency, markets_json, primary_airports_json, language, origin, lat, lon FROM destination_config',
      'SELECT slug, country_code, currency, timezone, holiday_calendar, primary_airports_json FROM origin_config',
      'SELECT key, value FROM global_config',
    ]);

    // Helper: parse result at index i into plain objects
    function rowsAt(idx: number): Record<string, any>[] {
      const result = batchResponse?.results?.[idx]?.response?.result;
      if (!result?.rows || !result?.cols) return [];
      const cols = (result.cols as Array<{ name: string }>).map((c: { name: string }) => c.name);
      return (result.rows as unknown[][]).map((row) => {
        const obj: Record<string, any> = {};
        for (let i = 0; i < cols.length; i++) {
          const cell = (row as any)[i];
          obj[cols[i]] = cell?.value ?? null;
        }
        return obj;
      });
    }

    // Build destination cache
    const destRows = rowsAt(0);
    const newDestCache: Record<string, DestinationConfig> = {};
    for (const row of destRows) {
      const markets: string[] = row.markets_json ? JSON.parse(row.markets_json) : [];
      const primaryAirports: string[] = row.primary_airports_json
        ? JSON.parse(row.primary_airports_json)
        : [];
      const cfg: DestinationConfig = {
        slug: row.slug,
        display_name: row.display_name,
        ref_id: row.ref_id || '',
        ref_path: row.ref_path || '',
        timezone: row.timezone || 'Asia/Tokyo',
        currency: row.currency || 'JPY',
        markets,
        primary_airports: primaryAirports,
        language: row.language || 'ja',
      };
      if (row.lat !== null && row.lon !== null) {
        cfg.coordinates = { lat: Number(row.lat), lon: Number(row.lon) };
      }
      newDestCache[row.slug] = cfg;
    }
    dbDestCache = newDestCache;

    // Build origin cache
    const originRows = rowsAt(1);
    const newOriginCache: Record<string, Record<string, string | string[]>> = {};
    for (const row of originRows) {
      const primaryAirports: string[] = row.primary_airports_json
        ? JSON.parse(row.primary_airports_json)
        : [];
      newOriginCache[row.slug] = {
        country_code: row.country_code || '',
        currency: row.currency || '',
        timezone: row.timezone || '',
        holiday_calendar: row.holiday_calendar || '',
        primary_airports: primaryAirports,
      };
    }
    dbOriginCache = newOriginCache;

    // Build global config cache
    const globalRows = rowsAt(2);
    const newGlobalCache: Record<string, string> = {};
    for (const row of globalRows) {
      newGlobalCache[row.key] = row.value;
    }
    dbGlobalCache = newGlobalCache;

    console.error(
      `[loader] Loaded ${Object.keys(newDestCache).length} destinations from DB`
    );
  } catch (err: any) {
    console.error('[loader] Could not load destination config from DB:', err.message);
    // Leave caches null — sync APIs will fall back to JSON file
  }
}

// ============================================================================
// Project root
// ============================================================================

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

// ============================================================================
// JSON file fallback
// ============================================================================

/**
 * Load destinations configuration from JSON file (backward-compat fallback).
 * No-op if DB cache is populated. No-op if file does not exist.
 */
export function loadDestinations(): DestinationsFile {
  if (destinationsCache) return destinationsCache;

  // If DB cache is populated, synthesize a DestinationsFile from it
  if (dbDestCache !== null) {
    destinationsCache = {
      version: 'db',
      destinations: { ...dbDestCache },
      default_destination: dbGlobalCache?.['default_destination'] || 'tokyo_2026',
    };
    return destinationsCache;
  }

  // Fallback: try JSON file
  const configPath = resolveRepoPath('data/destinations.json', 'Destinations config');
  if (!fs.existsSync(configPath)) {
    // Return an empty stub so callers do not crash; DB should be loaded first
    destinationsCache = { version: 'empty', destinations: {}, default_destination: 'tokyo_2026' };
    return destinationsCache;
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
  dbDestCache = null;
  dbOriginCache = null;
  dbGlobalCache = null;
  dbLoadAttempted = false;
}

// ============================================================================
// Destination Discovery APIs
// ============================================================================

/**
 * Get all available destination slugs.
 * Merges DB cache + JSON file cache.
 */
export function getAvailableDestinations(): string[] {
  // DB cache takes priority; JSON fallback merges in as well
  if (dbDestCache !== null) {
    const dbSlugs = Object.keys(dbDestCache);
    // Merge with any JSON-only entries (if JSON still exists during transition)
    const configPath = resolveRepoPath('data/destinations.json', 'Destinations config');
    if (fs.existsSync(configPath)) {
      const jsonDests = loadDestinations().destinations;
      const merged = new Set([...dbSlugs, ...Object.keys(jsonDests)]);
      return Array.from(merged);
    }
    return dbSlugs;
  }
  const config = loadDestinations();
  return Object.keys(config.destinations);
}

/**
 * Get destination configuration by slug.
 * Checks DB cache first, then JSON cache.
 */
export function getDestinationConfig(slug: string): DestinationConfig | null {
  if (dbDestCache !== null) {
    return dbDestCache[slug] || null;
  }
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
    const allDests = dbDestCache !== null
      ? Object.values(dbDestCache)
      : Object.values(loadDestinations().destinations);
    for (const dest of allDests) {
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
 * Checks DB global config first.
 */
export function getDefaultDestination(): string {
  if (dbGlobalCache !== null) {
    return dbGlobalCache['default_destination'] || 'tokyo_2026';
  }
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

// ============================================================================
// OTA Source Discovery APIs
// ============================================================================

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

// ============================================================================
// Inference APIs
// ============================================================================

/**
 * Infer OTA source_id from a URL by matching against base_url domains in config.
 * Returns 'unknown' if no match found.
 */
export function inferSourceIdFromUrl(url: string): string {
  const config = loadOtaSources();
  for (const [sourceId, source] of Object.entries(config.sources)) {
    if (!source.base_url) continue;
    try {
      const domain = new URL(source.base_url).hostname.replace(/^www\./, '');
      if (url.includes(domain)) return sourceId;
    } catch {
      continue;
    }
  }
  return 'unknown';
}

const REGION_MAP: Record<string, string> = {
  tokyo: 'tokyo',
  tyo: 'tokyo',
  osaka: 'kansai',
  kansai: 'kansai',
  kyoto: 'kansai',
  nagoya: 'nagoya',
  hokkaido: 'hokkaido',
  sapporo: 'hokkaido',
  okinawa: 'okinawa',
};

/**
 * Infer region from a destination slug (e.g., 'tokyo_2026' → 'tokyo').
 * Tries destination config ref_id first, then substring matching.
 */
export function inferRegionFromDestination(destination: string): string | undefined {
  // Try destination config first
  const destConfig = getDestinationConfig(destination);
  if (destConfig?.ref_id) {
    const refLower = destConfig.ref_id.toLowerCase();
    if (REGION_MAP[refLower]) return REGION_MAP[refLower];
  }

  // Fallback: substring matching
  const d = destination.toLowerCase();
  for (const [key, region] of Object.entries(REGION_MAP)) {
    if (d.includes(key)) return region;
  }
  return undefined;
}
