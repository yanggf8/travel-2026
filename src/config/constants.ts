/**
 * Skill Pack Configuration Constants
 *
 * Centralizes hardcoded values for multi-destination and multi-currency support.
 * Version: 1.0.0
 */

export const CONFIG_VERSION = '1.0.0';

/**
 * Default values - can be overridden per destination or travel plan.
 */
export const DEFAULTS = {
  /** Default passengers count */
  pax: 2,

  /** Default itinerary pace */
  pace: 'balanced' as const,

  /** Default project identifier for state files */
  project: 'travel-planner',

  /** Session types in order */
  sessionOrder: ['morning', 'afternoon', 'evening'] as const,
} as const;

/**
 * Pace levels for itinerary population.
 */
export const PACE_LEVELS = {
  relaxed: {
    activitiesPerSession: 1,
    description: 'One main activity per session, plenty of rest time',
  },
  balanced: {
    activitiesPerSession: 2,
    description: 'Two activities per session, moderate pace',
  },
  packed: {
    activitiesPerSession: 3,
    description: 'Maximum activities, early starts and late finishes',
  },
} as const;

export type PaceLevel = keyof typeof PACE_LEVELS;

/**
 * Day types and their session configurations.
 */
export const DAY_TYPE_SESSIONS = {
  arrival: {
    morning: { focus: 'Departure', available: false },
    afternoon: { focus: 'Flight & Arrival', available: true },
    evening: { focus: 'Hotel Check-in', available: true },
  },
  full: {
    morning: { focus: 'Morning activities', available: true },
    afternoon: { focus: 'Afternoon activities', available: true },
    evening: { focus: 'Evening activities', available: true },
  },
  departure: {
    morning: { focus: 'Pack & Checkout', available: true },
    afternoon: { focus: 'Final activities', available: true },
    evening: { focus: 'Airport Transfer', available: false },
  },
} as const;

/**
 * Supported currencies with display info.
 */
export const CURRENCIES = {
  TWD: { symbol: 'NT$', name: 'Taiwan Dollar', locale: 'zh-TW' },
  JPY: { symbol: '¥', name: 'Japanese Yen', locale: 'ja-JP' },
  USD: { symbol: '$', name: 'US Dollar', locale: 'en-US' },
} as const;

export type CurrencyCode = keyof typeof CURRENCIES;

/**
 * Activity ID generation prefix.
 */
export const ACTIVITY_ID_PREFIX = 'activity_';

/**
 * Schema versions for compatibility tracking.
 */
export const SCHEMA_VERSIONS = {
  travelPlan: '4.2.0',
  activity: '1.0.0',
  destinationRef: '1.0.0',
  skillContract: '1.0.0',
} as const;
