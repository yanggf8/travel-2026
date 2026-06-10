/**
 * TursoDbClient — DbClient implementation over the Turso HTTP pipeline.
 *
 * Reuses the same `scripts/turso-pipeline` client the repository layer uses
 * (loaded via require to bridge the src/scripts boundary, matching the
 * convention in turso-repository.ts and turso-service.ts).
 */

import type { DbClient, SqlArg } from './db-client';

/** Turso pipeline arg encoding (note: real → 'float'). */
type TursoArg =
  | { type: 'null' }
  | { type: 'text'; value: string }
  | { type: 'integer'; value: string }
  | { type: 'float'; value: number };

interface TursoPipelineResponse {
  results?: Array<{
    response?: {
      result?: {
        cols?: Array<{ name: string }>;
        rows?: Array<Array<{ value?: unknown } | null>>;
        affected_row_count?: number;
      };
    };
  }>;
}

interface PipelineClient {
  executeParams(sql: string, args: TursoArg[]): Promise<TursoPipelineResponse>;
}

function requirePipeline(): { TursoPipelineClient: new (opts?: unknown) => PipelineClient } {
  // Explicit `.ts` + cwd-relative path: resolves under both ts-node (CLI) and
  // vitest, matching turso-service.ts / shaping-service.ts convention.
  // eslint-disable-next-line @typescript-eslint/no-var-requires
  const path = require('node:path');
  return require(path.join(process.cwd(), 'scripts', 'turso-pipeline.ts'));
}

/** Translate a DbClient SqlArg into the pipeline's arg encoding. */
function toTursoArg(a: SqlArg): TursoArg {
  return a.type === 'real' ? { type: 'float', value: a.value } : a;
}

/** Map a pipeline result block (cols + rows) into plain objects. */
function resultToObjects(resp: TursoPipelineResponse): Record<string, unknown>[] {
  const result = resp?.results?.[0]?.response?.result;
  const cols = result?.cols;
  const rows = result?.rows;
  if (!cols || !rows) return [];
  const names = cols.map((c) => c.name);
  return rows.map((row) => {
    const obj: Record<string, unknown> = {};
    names.forEach((name, i) => {
      obj[name] = row[i]?.value ?? null;
    });
    return obj;
  });
}

export class TursoDbClient implements DbClient {
  private client: PipelineClient;

  constructor(client?: PipelineClient) {
    if (client) {
      this.client = client;
    } else {
      const { TursoPipelineClient } = requirePipeline();
      this.client = new TursoPipelineClient();
    }
  }

  async queryOne<T = Record<string, unknown>>(sql: string, args: SqlArg[] = []): Promise<T | null> {
    const rows = await this.queryMany<T>(sql, args);
    return rows.length > 0 ? rows[0] : null;
  }

  async queryMany<T = Record<string, unknown>>(sql: string, args: SqlArg[] = []): Promise<T[]> {
    const resp = await this.client.executeParams(sql, args.map(toTursoArg));
    return resultToObjects(resp) as T[];
  }

  async execute(sql: string, args: SqlArg[] = []): Promise<{ affected: number }> {
    const resp = await this.client.executeParams(sql, args.map(toTursoArg));
    const affected = resp?.results?.[0]?.response?.result?.affected_row_count ?? 0;
    return { affected };
  }
}
