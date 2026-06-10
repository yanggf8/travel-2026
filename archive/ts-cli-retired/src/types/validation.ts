/**
 * Input validation utilities for CLI arguments.
 *
 * Provides simple sanity checks for common input types:
 * - ISO dates (YYYY-MM-DD)
 * - Positive integers
 * - Time strings (HH:MM)
 */

import { Result } from './result';

/**
 * Validate ISO-8601 date format (YYYY-MM-DD).
 * Also checks that the date is actually valid (not Feb 30, etc.)
 */
export function validateIsoDate(input: string, fieldName = 'date'): Result<string> {
  if (!input || typeof input !== 'string') {
    return Result.err(`${fieldName} is required`);
  }

  // Check format
  const match = input.match(/^(\d{4})-(\d{2})-(\d{2})$/);
  if (!match) {
    return Result.err(`${fieldName} must be YYYY-MM-DD format (got: "${input}")`);
  }

  // Check date validity
  const [, year, month, day] = match;
  const date = new Date(`${year}-${month}-${day}T00:00:00`);
  if (
    isNaN(date.getTime()) ||
    date.getFullYear() !== parseInt(year, 10) ||
    date.getMonth() + 1 !== parseInt(month, 10) ||
    date.getDate() !== parseInt(day, 10)
  ) {
    return Result.err(`${fieldName} is not a valid date: "${input}"`);
  }

  return Result.ok(input);
}

/**
 * Validate positive integer.
 */
export function validatePositiveInt(input: string, fieldName = 'number'): Result<number> {
  if (!input || typeof input !== 'string') {
    return Result.err(`${fieldName} is required`);
  }

  const num = parseInt(input, 10);
  if (!Number.isFinite(num)) {
    return Result.err(`${fieldName} must be a number (got: "${input}")`);
  }

  if (num <= 0) {
    return Result.err(`${fieldName} must be positive (got: ${num})`);
  }

  return Result.ok(num);
}

/**
 * Validate non-negative integer (including zero).
 */
export function validateNonNegativeInt(input: string, fieldName = 'number'): Result<number> {
  if (!input || typeof input !== 'string') {
    return Result.err(`${fieldName} is required`);
  }

  const num = parseInt(input, 10);
  if (!Number.isFinite(num)) {
    return Result.err(`${fieldName} must be a number (got: "${input}")`);
  }

  if (num < 0) {
    return Result.err(`${fieldName} must be non-negative (got: ${num})`);
  }

  return Result.ok(num);
}

/**
 * Validate time format (HH:MM, 24-hour).
 */
export function validateTime(input: string, fieldName = 'time'): Result<string> {
  if (!input || typeof input !== 'string') {
    return Result.err(`${fieldName} is required`);
  }

  const match = input.match(/^([01]?\d|2[0-3]):([0-5]\d)$/);
  if (!match) {
    return Result.err(`${fieldName} must be HH:MM format (got: "${input}")`);
  }

  // Normalize to HH:MM (add leading zero if needed)
  const [, hours, minutes] = match;
  const normalized = `${hours.padStart(2, '0')}:${minutes}`;

  return Result.ok(normalized);
}

/**
 * Validate date range (start <= end).
 */
export function validateDateRange(
  startDate: string,
  endDate: string
): Result<{ start: string; end: string; days: number }> {
  const startResult = validateIsoDate(startDate, 'start date');
  if (!startResult.ok) return startResult;

  const endResult = validateIsoDate(endDate, 'end date');
  if (!endResult.ok) return endResult;

  const start = new Date(startDate);
  const end = new Date(endDate);

  if (start > end) {
    return Result.err(`Start date (${startDate}) cannot be after end date (${endDate})`);
  }

  const days = Math.ceil((end.getTime() - start.getTime()) / (1000 * 60 * 60 * 24)) + 1;

  return Result.ok({ start: startDate, end: endDate, days });
}
