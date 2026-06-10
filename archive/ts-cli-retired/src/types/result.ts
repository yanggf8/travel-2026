/**
 * Result type for consistent error handling across the codebase.
 *
 * Usage:
 *   - Functions that can fail return Result<T>
 *   - Use Result.ok(value) for success
 *   - Use Result.err(message) for failure
 *   - Check with result.ok before accessing result.value
 *
 * @example
 * function divide(a: number, b: number): Result<number> {
 *   if (b === 0) return Result.err('Division by zero');
 *   return Result.ok(a / b);
 * }
 *
 * const result = divide(10, 2);
 * if (result.ok) {
 *   console.log(result.value); // 5
 * } else {
 *   console.error(result.error); // Never reaches here
 * }
 */

export type Result<T, E = string> =
  | { ok: true; value: T }
  | { ok: false; error: E };

export const Result = {
  ok<T>(value: T): Result<T, never> {
    return { ok: true, value };
  },

  err<E = string>(error: E): Result<never, E> {
    return { ok: false, error };
  },

  /**
   * Wrap a function that may throw into one that returns Result.
   */
  wrap<T, Args extends unknown[]>(
    fn: (...args: Args) => T
  ): (...args: Args) => Result<T> {
    return (...args: Args) => {
      try {
        return Result.ok(fn(...args));
      } catch (e) {
        return Result.err(e instanceof Error ? e.message : String(e));
      }
    };
  },

  /**
   * Wrap an async function that may throw into one that returns Result.
   */
  wrapAsync<T, Args extends unknown[]>(
    fn: (...args: Args) => Promise<T>
  ): (...args: Args) => Promise<Result<T>> {
    return async (...args: Args) => {
      try {
        return Result.ok(await fn(...args));
      } catch (e) {
        return Result.err(e instanceof Error ? e.message : String(e));
      }
    };
  },

  /**
   * Unwrap a Result, throwing if it's an error.
   */
  unwrap<T>(result: Result<T>): T {
    if (result.ok) return result.value;
    throw new Error(result.error);
  },

  /**
   * Unwrap a Result with a default value if it's an error.
   */
  unwrapOr<T>(result: Result<T>, defaultValue: T): T {
    return result.ok ? result.value : defaultValue;
  },

  /**
   * Map over a successful Result.
   */
  map<T, U>(result: Result<T>, fn: (value: T) => U): Result<U> {
    return result.ok ? Result.ok(fn(result.value)) : result;
  },

  /**
   * Collect multiple Results into a single Result of array.
   * Returns first error if any Result is an error.
   */
  all<T>(results: Result<T>[]): Result<T[]> {
    const values: T[] = [];
    for (const result of results) {
      if (!result.ok) return result;
      values.push(result.value);
    }
    return Result.ok(values);
  },
};

/**
 * Type guard for successful Result.
 */
export function isOk<T, E>(result: Result<T, E>): result is { ok: true; value: T } {
  return result.ok;
}

/**
 * Type guard for failed Result.
 */
export function isErr<T, E>(result: Result<T, E>): result is { ok: false; error: E } {
  return !result.ok;
}
