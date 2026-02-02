/**
 * OTA Scraper Interface
 *
 * Defines the contract for all OTA scrapers.
 * Scrapers must normalize results to CanonicalOffer format.
 */

/**
 * Common scraper configuration from ota-sources.json
 */
export interface OtaScraperConfig {
  sourceId: string;
  displayName: string;
  baseUrl: string;
  currency: string;
  rateLimit: {
    requestsPerMinute: number;
  };
  promoCodes?: PromoCode[];
}

export interface PromoCode {
  code: string;
  discount: number;
  currency: string;
  minPurchase: number;
  validDays?: string[];
  expires: string | null;
}

/**
 * Search parameters for OTA queries
 */
export interface OtaSearchParams {
  destination: string;
  startDate: string;
  endDate: string;
  pax: number;
  flexible?: boolean;
  productTypes?: ('package' | 'flight' | 'hotel')[];
  maxResults?: number;
}

/**
 * Canonical flight segment
 */
export interface FlightSegment {
  flightNumber: string;
  airline: string;
  airlineCode?: string;
  departureAirport: string;
  departureCode: string;
  departureTime: string;
  arrivalAirport: string;
  arrivalCode: string;
  arrivalTime: string;
  date: string;
}

/**
 * Canonical hotel info
 */
export interface HotelInfo {
  name: string;
  slug?: string;
  area: string;
  starRating?: number;
  access?: string[];
  amenities?: string[];
  roomType?: string;
}

/**
 * Date-specific pricing entry
 */
export interface DatePricing {
  date: string;
  pricePerPerson: number;
  priceTotal?: number;
  availability: 'available' | 'sold_out' | 'limited' | 'unknown';
  notes?: string;
}

/**
 * Canonical offer (normalized from any OTA)
 */
export interface CanonicalOffer {
  id: string;
  sourceId: string;
  type: 'package' | 'flight' | 'hotel';
  title: string;
  url: string;
  currency: string;
  pricePerPerson: number;
  priceTotal?: number;
  availability: 'available' | 'sold_out' | 'limited' | 'unknown';

  // Flight details (for package or flight type)
  flight?: {
    outbound: FlightSegment;
    return: FlightSegment;
  };

  // Hotel details (for package or hotel type)
  hotel?: HotelInfo;

  // Package inclusions
  includes?: string[];

  // Date-specific pricing calendar
  datePricing?: DatePricing[];

  // Best value date
  bestValue?: {
    date: string;
    pricePerPerson: number;
    priceTotal: number;
  };

  // Metadata
  scrapedAt: string;
  rawData?: unknown;
}

/**
 * Scrape result with provenance
 */
export interface ScrapeResult {
  success: boolean;
  offers: CanonicalOffer[];
  provenance: {
    sourceId: string;
    scrapedAt: string;
    offersFound: number;
    searchParams: OtaSearchParams;
    duration_ms: number;
  };
  errors: string[];
  warnings: string[];
}

/**
 * Abstract scraper interface
 * Each OTA scraper implements this interface
 */
export interface IOtaScraper {
  readonly sourceId: string;
  readonly config: OtaScraperConfig;

  /**
   * Search for offers matching the given parameters
   */
  search(params: OtaSearchParams): Promise<ScrapeResult>;

  /**
   * Scrape a specific product URL
   */
  scrapeProduct(url: string): Promise<ScrapeResult>;

  /**
   * Check if scraper supports the given destination
   */
  supportsDestination(destination: string): boolean;

  /**
   * Get available promo codes for current date
   */
  getActivePromoCodes(): PromoCode[];
}

/**
 * Scraper registry for managing multiple OTA scrapers
 */
export interface IScraperRegistry {
  /**
   * Register a scraper implementation
   */
  register(scraper: IOtaScraper): void;

  /**
   * Get scraper by source ID
   */
  get(sourceId: string): IOtaScraper | undefined;

  /**
   * Get all registered scrapers
   */
  getAll(): IOtaScraper[];

  /**
   * Get scrapers that support a destination
   */
  getForDestination(destination: string): IOtaScraper[];

  /**
   * Search across all scrapers in parallel
   */
  searchAll(params: OtaSearchParams): Promise<ScrapeResult[]>;
}
