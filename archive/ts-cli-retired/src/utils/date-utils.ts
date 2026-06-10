/**
 * Shared date utilities for travel planning CLI tools.
 */

/**
 * Add days to a YYYY-MM-DD date string. Returns YYYY-MM-DD.
 */
export function addDays(dateStr: string, days: number): string {
  const [y, m, d] = dateStr.split('-').map(Number);
  const date = new Date(y, m - 1, d + days);
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
}

/**
 * Get the day of the week (0=Sunday, 6=Saturday) for a YYYY-MM-DD date string.
 */
export function getDayOfWeek(dateStr: string): number {
  const [y, m, d] = dateStr.split('-').map(Number);
  return new Date(y, m - 1, d).getDay();
}

/**
 * Calculate number of hotel nights from trip days.
 */
export function calculateNights(days: number): number {
  return days - 1;
}
