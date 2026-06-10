/**
 * DbClient — fine-grained DB operations for StateManagerV2 (ADR-001).
 *
 * The target pattern: each StateManager method does one targeted SELECT to
 * validate preconditions, then one targeted parameterized UPDATE/INSERT that
 * writes exactly what changed. No in-memory plan object, no coarse flush.
 *
 * This interface is the seam. `TursoDbClient` implements it over the Turso
 * HTTP pipeline; tests run it against a real Turso DB (no mocks — see ADR-001
 * "Testing"). See src/skills/travel-shared/references/architecture-decisions.md.
 */

/**
 * A bound SQL argument. Mirrors the Turso HTTP pipeline arg encoding.
 *
 * IMPORTANT: integer values must be passed as strings
 * (`{ type: 'integer', value: '1' }`) — the Turso pipeline rejects numeric
 * JSON values for integer columns.
 */
export type SqlArg =
  | { type: 'null' }
  | { type: 'text'; value: string }
  | { type: 'integer'; value: string }
  | { type: 'real'; value: number };

/** Convenience constructors for SqlArg (null-safe). */
export const arg = {
  text(value: string | null | undefined): SqlArg {
    return value === null || value === undefined ? { type: 'null' } : { type: 'text', value: String(value) };
  },
  int(value: number | string | null | undefined): SqlArg {
    if (value === null || value === undefined || value === '') return { type: 'null' };
    return { type: 'integer', value: String(Math.trunc(Number(value))) };
  },
  real(value: number | null | undefined): SqlArg {
    return value === null || value === undefined || !Number.isFinite(value)
      ? { type: 'null' }
      : { type: 'real', value };
  },
};

export interface DbClient {
  /** Read a single row as a plain object, or null if no row matched. */
  queryOne<T = Record<string, unknown>>(sql: string, args?: SqlArg[]): Promise<T | null>;

  /** Read all matching rows as plain objects. */
  queryMany<T = Record<string, unknown>>(sql: string, args?: SqlArg[]): Promise<T[]>;

  /** Execute a write; returns the number of affected rows. */
  execute(sql: string, args?: SqlArg[]): Promise<{ affected: number }>;
}
