/**
 * Base Scraper
 *
 * Abstract base class for OTA scrapers with common utilities.
 */

import {
  IOtaScraper,
  OtaScraperConfig,
  OtaSearchParams,
  ScrapeResult,
  CanonicalOffer,
  PromoCode,
} from './types';
import { getOtaSourceConfig, getDestinationConfig } from '../config/loader';

export abstract class BaseScraper implements IOtaScraper {
  readonly sourceId: string;
  readonly config: OtaScraperConfig;

  constructor(sourceId: string) {
    this.sourceId = sourceId;

    const otaConfig = getOtaSourceConfig(sourceId);
    if (!otaConfig) {
      throw new Error(`OTA source not found in registry: ${sourceId}`);
    }

    this.config = {
      sourceId: otaConfig.source_id,
      displayName: otaConfig.display_name,
      baseUrl: otaConfig.base_url,
      currency: otaConfig.currency,
      rateLimit: {
        requestsPerMinute: otaConfig.rate_limit?.requests_per_minute || 10,
      },
      promoCodes: otaConfig.promo_codes?.map((p) => ({
        code: p.code,
        discount: p.discount,
        currency: p.currency,
        minPurchase: p.min_purchase,
        validDays: p.valid_days,
        expires: p.expires,
      })),
    };
  }

  abstract search(params: OtaSearchParams): Promise<ScrapeResult>;
  abstract scrapeProduct(url: string): Promise<ScrapeResult>;

  supportsDestination(destination: string): boolean {
    const destConfig = getDestinationConfig(destination);
    if (!destConfig) return false;

    const otaConfig = getOtaSourceConfig(this.sourceId);
    if (!otaConfig) return false;

    // Check if markets overlap
    return destConfig.markets.some((m) => otaConfig.markets.includes(m));
  }

  getActivePromoCodes(): PromoCode[] {
    if (!this.config.promoCodes) return [];

    const today = new Date();
    const dayName = today.toLocaleDateString('en-US', { weekday: 'long' });

    return this.config.promoCodes.filter((promo) => {
      // Check expiration
      if (promo.expires && new Date(promo.expires) < today) {
        return false;
      }

      // Check valid days
      if (promo.validDays && promo.validDays.length > 0) {
        return promo.validDays.includes(dayName);
      }

      return true;
    });
  }

  /**
   * Create empty result for error cases
   */
  protected createErrorResult(
    params: OtaSearchParams,
    errors: string[],
    startTime: number
  ): ScrapeResult {
    return {
      success: false,
      offers: [],
      provenance: {
        sourceId: this.sourceId,
        scrapedAt: new Date().toISOString(),
        offersFound: 0,
        searchParams: params,
        duration_ms: Date.now() - startTime,
      },
      errors,
      warnings: [],
    };
  }

  /**
   * Create success result
   */
  protected createSuccessResult(
    params: OtaSearchParams,
    offers: CanonicalOffer[],
    startTime: number,
    warnings: string[] = []
  ): ScrapeResult {
    return {
      success: true,
      offers,
      provenance: {
        sourceId: this.sourceId,
        scrapedAt: new Date().toISOString(),
        offersFound: offers.length,
        searchParams: params,
        duration_ms: Date.now() - startTime,
      },
      errors: [],
      warnings,
    };
  }

  /**
   * Generate canonical offer ID
   */
  protected generateOfferId(productCode: string): string {
    return `${this.sourceId}_${productCode}`;
  }

  /**
   * Rate limiting helper
   */
  protected async rateLimit(): Promise<void> {
    const delayMs = (60 * 1000) / this.config.rateLimit.requestsPerMinute;
    await new Promise((resolve) => setTimeout(resolve, delayMs));
  }
}
