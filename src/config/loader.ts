/**
 * Configuration Loader
 *
 * Loads destinations and OTA sources configuration.
 * Provides discovery APIs for skill composition.
 *
 * Destination config is stored in Turso DB (destination_config, origin_config, global_config).
 * Call loadDestinationConfigFromDb() at startup to populate the in-memory caches.
 * All sync APIs require the DB cache. Missing DB rows are configuration errors.
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

/** API-shaped caches synthesized from Turso rows. */
let destinationsCache: DestinationsFile | null = null;
let otaSourcesCache: OtaSourcesFile | null = null;
let projectRootCache: string | null = null;

/** DB-populated caches (populated by loadDestinationConfigFromDb) */
let dbDestCache: Record<string, DestinationConfig> | null = null;
let dbOriginCache: Record<string, Record<string, string | string[]>> | null = null;
let dbGlobalCache: Record<string, string> | null = null;
let dbOtaSourcesCache: Map<string, OtaSourceConfig> | null = null;
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
  if (dbLoadAttempted) {
    missingDbConfig('Destination config DB load was attempted but no cache was populated');
  }
  dbLoadAttempted = true;

  try {
    const { TursoPipelineClient } = requirePipeline();
    const client = new TursoPipelineClient();

    const batchResponse = await client.executeBatch([
      'SELECT slug, display_name, ref_id, ref_path, timezone, currency, markets_json, primary_airports_json, language, origin, lat, lon FROM destination_config',
      'SELECT slug, country_code, currency, timezone, primary_airports_json FROM origin_config',
      'SELECT key, value FROM global_config',
      'SELECT source_id, name, type_json, status, scraper_script, regions_json, url_template, notes FROM ota_sources',
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

    // Build OTA sources cache
    const otaRows = rowsAt(3);
    const newOtaCache = new Map<string, OtaSourceConfig>();
    for (const row of otaRows) {
      const types: ('package' | 'flight' | 'hotel')[] = row.type_json ? JSON.parse(row.type_json) : [];
      const regions: string[] | undefined = row.regions_json ? JSON.parse(row.regions_json) : undefined;
      const cfg: OtaSourceConfig = {
        source_id: row.source_id,
        display_name: row.name,
        display_name_en: row.name,
        types,
        base_url: row.url_template || '',
        markets: [],
        currency: 'TWD',
        supported: row.status === 'active',
        scraper_script: row.scraper_script || null,
        notes: row.notes || undefined,
      };
      if (regions) (cfg as any).regions = regions;
      newOtaCache.set(row.source_id, cfg);
    }
    dbOtaSourcesCache = newOtaCache;

    console.error(
      `[loader] Loaded ${Object.keys(newDestCache).length} destinations, ${newOtaCache.size} OTA sources from DB`
    );
  } catch (err: any) {
    throw new Error(`Could not load destination config from Turso: ${err.message}. Run npm run db:migrate:turso to create and seed destination_config/ota_sources.`);
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
// DB-backed config access
// ============================================================================

function missingDbConfig(message: string): never {
  throw new Error(`${message}. Run npm run db:migrate:turso to create/seed destination_config, origin_config, global_config, and ota_sources before running this command.`);
}

/**
 * Load destinations configuration from Turso-populated cache.
 */
export function loadDestinations(): DestinationsFile {
  if (destinationsCache) return destinationsCache;

  if (dbDestCache === null) {
    missingDbConfig('Destination config cache is empty');
  }
  if (Object.keys(dbDestCache).length === 0) {
    missingDbConfig('Turso destination_config has no rows');
  }

  destinationsCache = {
    version: 'db',
    destinations: { ...dbDestCache },
    default_destination: dbGlobalCache?.['default_destination'] || missingDbConfig('Turso global_config.default_destination is missing'),
  };
  return destinationsCache;
}

/**
 * Load OTA sources configuration from Turso-populated cache.
 */
export function loadOtaSources(): OtaSourcesFile {
  if (otaSourcesCache) return otaSourcesCache;

  if (dbOtaSourcesCache === null) {
    missingDbConfig('OTA sources cache is empty');
  }
  if (dbOtaSourcesCache.size === 0) {
    missingDbConfig('Turso ota_sources has no rows');
  }

  const sources: Record<string, OtaSourceConfig> = {};
  for (const [id, cfg] of dbOtaSourcesCache) {
    sources[id] = cfg;
  }
  otaSourcesCache = { version: 'db', sources };
  return otaSourcesCache;
}

/**
 * Get OTA source configuration by ID.
 * Checks DB cache directly for efficiency, falls back through loadOtaSources().
 */
export function getOtaSource(sourceId: string): OtaSourceConfig | undefined {
  if (dbOtaSourcesCache !== null) {
    return dbOtaSourcesCache.get(sourceId) ?? undefined;
  }
  const config = loadOtaSources();
  return config.sources[sourceId] ?? undefined;
}

/**
 * Get all OTA source configs as an array.
 * Checks DB cache directly for efficiency, falls back through loadOtaSources().
 */
export function getAllOtaSources(): OtaSourceConfig[] {
  if (dbOtaSourcesCache !== null) {
    return Array.from(dbOtaSourcesCache.values());
  }
  return Object.values(loadOtaSources().sources);
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
  dbOtaSourcesCache = null;
  dbLoadAttempted = false;
}

// ============================================================================
// Destination Discovery APIs
// ============================================================================

/**
 * Get all available destination slugs.
 * Reads the Turso-backed cache.
 */
export function getAvailableDestinations(): string[] {
  const config = loadDestinations();
  return Object.keys(config.destinations);
}

/**
 * Get destination configuration by slug.
 * Checks the Turso-backed cache.
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
    const allDests = Object.values(loadDestinations().destinations);
    for (const dest of allDests) {
      if (slug.toLowerCase().includes(dest.ref_id.toLowerCase())) {
        const refPath = resolveRepoPath(dest.ref_path, `Destination ref_path (${dest.slug})`);
        return fs.existsSync(refPath) ? refPath : null;
      }
    }
    return null;
  }

  if (!destConfig.ref_path) return null;
  const refPath = resolveRepoPath(destConfig.ref_path, `Destination ref_path (${destConfig.slug})`);
  return fs.existsSync(refPath) ? refPath : null;
}

/**
 * Get default destination slug.
 * Checks DB global config first.
 */
export function getDefaultDestination(): string {
  if (dbGlobalCache !== null) {
    const defaultDestination = dbGlobalCache['default_destination'];
    if (!defaultDestination) missingDbConfig('Turso global_config.default_destination is missing');
    return defaultDestination;
  }
  return loadDestinations().default_destination;
}

/**
 * Get currency for a destination.
 */
export function getDestinationCurrency(slug: string): string {
  const destConfig = getDestinationConfig(slug);
  return destConfig?.currency || 'JPY';
}

export interface OriginConfig {
  slug: string;
  country_code?: string;
  currency?: string;
  timezone?: string;
  primary_airports?: string[];
}

/**
 * Get origin configuration by slug (e.g., 'taiwan').
 * Returns undefined if DB has not been loaded yet or slug not found.
 */
export function getOriginConfig(slug: string): OriginConfig | undefined {
  if (dbOriginCache === null) return undefined;
  const row = dbOriginCache[slug];
  if (!row) return undefined;
  return {
    slug,
    country_code: (row.country_code as string) || undefined,
    currency: (row.currency as string) || undefined,
    timezone: (row.timezone as string) || undefined,
    primary_airports: Array.isArray(row.primary_airports) ? (row.primary_airports as string[]) : undefined,
  };
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
