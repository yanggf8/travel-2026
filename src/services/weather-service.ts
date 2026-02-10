/**
 * Weather Service
 *
 * Fetches weather forecasts from Open-Meteo (free, no API key).
 * Returns DayWeather[] for itinerary date range.
 */

import { execSync } from 'node:child_process';
import { getDestinationConfig } from '../config/loader';
import type { DayWeather } from '../state/types';

/** WMO Weather interpretation codes → human-readable labels */
const WMO_LABELS: Record<number, string> = {
  0: 'Clear sky',
  1: 'Mainly clear',
  2: 'Partly cloudy',
  3: 'Overcast',
  45: 'Foggy',
  48: 'Depositing rime fog',
  51: 'Light drizzle',
  53: 'Moderate drizzle',
  55: 'Dense drizzle',
  56: 'Light freezing drizzle',
  57: 'Dense freezing drizzle',
  61: 'Slight rain',
  63: 'Moderate rain',
  65: 'Heavy rain',
  66: 'Light freezing rain',
  67: 'Heavy freezing rain',
  71: 'Slight snowfall',
  73: 'Moderate snowfall',
  75: 'Heavy snowfall',
  77: 'Snow grains',
  80: 'Slight rain showers',
  81: 'Moderate rain showers',
  82: 'Violent rain showers',
  85: 'Slight snow showers',
  86: 'Heavy snow showers',
  95: 'Thunderstorm',
  96: 'Thunderstorm with slight hail',
  99: 'Thunderstorm with heavy hail',
};

interface OpenMeteoDaily {
  time: string[];
  temperature_2m_max: number[];
  temperature_2m_min: number[];
  precipitation_probability_max: number[];
  weather_code: number[];
}

interface OpenMeteoResponse {
  daily: OpenMeteoDaily;
}

/**
 * Fetch weather forecast for a destination and date range.
 *
 * @param startDate - ISO date string (YYYY-MM-DD)
 * @param endDate - ISO date string (YYYY-MM-DD)
 * @param destination - Destination slug (e.g. "tokyo_2026")
 * @returns DayWeather[] for each day in range, or empty array if dates outside forecast window
 */
export async function fetchWeather(
  startDate: string,
  endDate: string,
  destination: string
): Promise<DayWeather[]> {
  const config = getDestinationConfig(destination);
  if (!config) {
    throw new Error(`Destination not found: ${destination}`);
  }
  if (!config.coordinates) {
    throw new Error(`No coordinates configured for ${destination}. Add coordinates to data/destinations.json.`);
  }

  const { lat, lon } = config.coordinates;
  const tz = config.timezone || 'Asia/Tokyo';

  // Check forecast window (Open-Meteo supports up to 16 days ahead)
  const now = new Date();
  const start = new Date(startDate);
  const daysAhead = Math.ceil((start.getTime() - now.getTime()) / (1000 * 60 * 60 * 24));
  if (daysAhead > 16) {
    console.error(`  [weather] Dates ${startDate}–${endDate} are ${daysAhead} days ahead — outside 16-day forecast window. Skipping.`);
    return [];
  }

  const url = `https://api.open-meteo.com/v1/forecast`
    + `?latitude=${lat}&longitude=${lon}`
    + `&daily=temperature_2m_max,temperature_2m_min,precipitation_probability_max,weather_code`
    + `&timezone=${encodeURIComponent(tz)}`
    + `&start_date=${startDate}&end_date=${endDate}`;

  let data: OpenMeteoResponse;
  try {
    // Use curl — Node's TCP stack may not resolve in some environments (e.g. WSL2)
    const body = execSync(`curl -sf "${url}"`, { encoding: 'utf-8', timeout: 15000 });
    data = JSON.parse(body) as OpenMeteoResponse;
  } catch (e: any) {
    throw new Error(`Open-Meteo API request failed: ${e.message}`);
  }

  if (!data.daily?.time) {
    throw new Error('No daily forecast data in Open-Meteo response');
  }

  const sourcedAt = new Date().toISOString();
  const results: DayWeather[] = [];

  for (let i = 0; i < data.daily.time.length; i++) {
    const code = data.daily.weather_code[i];
    results.push({
      temp_high_c: data.daily.temperature_2m_max[i],
      temp_low_c: data.daily.temperature_2m_min[i],
      precipitation_pct: data.daily.precipitation_probability_max[i],
      weather_code: code,
      weather_label: WMO_LABELS[code] || `WMO ${code}`,
      source_id: 'open_meteo',
      sourced_at: sourcedAt,
    });
  }

  return results;
}
