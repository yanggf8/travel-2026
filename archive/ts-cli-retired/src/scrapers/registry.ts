/**
 * Scraper Registry
 *
 * Manages OTA scraper instances and provides unified search interface.
 */

import {
  IOtaScraper,
  IScraperRegistry,
  OtaSearchParams,
  ScrapeResult,
} from './types';
import { getOtaSourceConfig, getSupportedOtaSources } from '../config/loader';

export class ScraperRegistry implements IScraperRegistry {
  private scrapers: Map<string, IOtaScraper> = new Map();

  register(scraper: IOtaScraper): void {
    if (this.scrapers.has(scraper.sourceId)) {
      console.warn(`Scraper ${scraper.sourceId} already registered, replacing...`);
    }
    this.scrapers.set(scraper.sourceId, scraper);
  }

  get(sourceId: string): IOtaScraper | undefined {
    return this.scrapers.get(sourceId);
  }

  getAll(): IOtaScraper[] {
    return Array.from(this.scrapers.values());
  }

  getForDestination(destination: string): IOtaScraper[] {
    return this.getAll().filter((s) => s.supportsDestination(destination));
  }

  async searchAll(params: OtaSearchParams): Promise<ScrapeResult[]> {
    const scrapers = params.destination
      ? this.getForDestination(params.destination)
      : this.getAll();

    if (scrapers.length === 0) {
      return [
        {
          success: false,
          offers: [],
          provenance: {
            sourceId: 'registry',
            scrapedAt: new Date().toISOString(),
            offersFound: 0,
            searchParams: params,
            duration_ms: 0,
          },
          errors: ['No scrapers available for destination'],
          warnings: [],
        },
      ];
    }

    // Run scrapers in parallel with error handling
    const results = await Promise.allSettled(
      scrapers.map((scraper) => scraper.search(params))
    );

    return results.map((result, idx) => {
      if (result.status === 'fulfilled') {
        return result.value;
      } else {
        return {
          success: false,
          offers: [],
          provenance: {
            sourceId: scrapers[idx].sourceId,
            scrapedAt: new Date().toISOString(),
            offersFound: 0,
            searchParams: params,
            duration_ms: 0,
          },
          errors: [result.reason?.message || 'Unknown error'],
          warnings: [],
        };
      }
    });
  }
}

/**
 * Global scraper registry instance
 */
export const globalRegistry = new ScraperRegistry();

/**
 * Get a list of supported OTA source IDs
 */
export function getRegisteredScrapers(): string[] {
  return Array.from(globalRegistry.getAll().map((s) => s.sourceId));
}

/**
 * Check if an OTA has a registered scraper
 */
export function hasScraperFor(sourceId: string): boolean {
  return globalRegistry.get(sourceId) !== undefined;
}

/**
 * Get scrapers available for a market (e.g., 'TW')
 */
export function getScrapersForMarket(market: string): IOtaScraper[] {
  return globalRegistry.getAll().filter((scraper) => {
    const config = getOtaSourceConfig(scraper.sourceId);
    return config?.markets.includes(market);
  });
}
