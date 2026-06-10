import fs from 'node:fs';
import path from 'node:path';

type TursoPipelineRequest = {
  type: 'execute';
  stmt: {
    sql: string;
    args?: TursoArg[];
  };
};

export type TursoPipelineResponse = {
  results?: Array<{
    response?: {
      result?: {
        cols?: Array<{ name: string }>;
        rows?: unknown[];
        affected_row_count?: number;
        last_insert_rowid?: string | number;
      };
      error?: unknown;
    };
  }>;
};

function loadDotEnvIfPresent(envPath = path.join(process.cwd(), '.env')): void {
  if (!fs.existsSync(envPath)) return;
  const raw = fs.readFileSync(envPath, 'utf-8');
  for (const line of raw.split('\n')) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith('#')) continue;
    const eq = trimmed.indexOf('=');
    if (eq === -1) continue;
    const key = trimmed.slice(0, eq).trim();
    let value = trimmed.slice(eq + 1).trim();
    if (
      (value.startsWith('"') && value.endsWith('"')) ||
      (value.startsWith("'") && value.endsWith("'"))
    ) {
      value = value.slice(1, -1);
    }
    if (!process.env[key]) process.env[key] = value;
  }
}

function requireEnv(name: string): string {
  const v = process.env[name];
  if (!v) throw new Error(`Missing required env var: ${name}`);
  return v;
}

export type TursoClientOptions = {
  endpoint?: string;
  tokenEnv?: string;
};

export type TursoArg =
  | { type: 'null' }
  | { type: 'text'; value: string }
  | { type: 'integer'; value: string }
  | { type: 'float'; value: number };

export function tursoText(value: string | null | undefined): TursoArg {
  return value === null || value === undefined
    ? { type: 'null' }
    : { type: 'text', value: String(value) };
}

export function tursoInt(value: number | string | null | undefined): TursoArg {
  if (value === null || value === undefined || value === '') return { type: 'null' };
  return { type: 'integer', value: String(Math.trunc(Number(value))) };
}

export function tursoFloat(value: number | null | undefined): TursoArg {
  return value === null || value === undefined || !Number.isFinite(value)
    ? { type: 'null' }
    : { type: 'float', value };
}

export class TursoPipelineClient {
  private endpoint: string;
  private tokenEnv: string;

  constructor(opts: TursoClientOptions = {}) {
    this.loadEnv();
    // Support both TURSO_HTTP_ENDPOINT (explicit) and TURSO_URL (standard, auto-derived)
    const envEndpoint = process.env.TURSO_HTTP_ENDPOINT
      || (process.env.TURSO_URL
        ? process.env.TURSO_URL.replace('libsql://', 'https://') + '/v2/pipeline'
        : undefined);

    this.endpoint = opts.endpoint || envEndpoint
      || 'https://travel-2026-yanggf8.aws-ap-northeast-1.turso.io/v2/pipeline';
    this.tokenEnv = opts.tokenEnv || 'TURSO_TOKEN';
  }

  loadEnv(): void {
    loadDotEnvIfPresent();
  }

  async execute(sql: string): Promise<TursoPipelineResponse> {
    const token = requireEnv(this.tokenEnv);
    const body = {
      requests: [{ type: 'execute', stmt: { sql } } satisfies TursoPipelineRequest],
    };

    const res = await fetch(this.endpoint, {
      method: 'POST',
      headers: {
        Authorization: `Bearer ${token}`,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify(body),
    });

    if (!res.ok) {
      const text = await res.text().catch(() => '');
      throw new Error(`Turso HTTP error ${res.status} at ${this.endpoint}: ${text || res.statusText}`);
    }

    const json = (await res.json()) as any;

    // Surface any error in the response body (Turso can put error at top level or under results[0].response.error)
    const topLevelError = json?.error;
    if (topLevelError) {
      throw new Error(`Turso pipeline error: ${JSON.stringify(topLevelError)}`);
    }
    const firstResultError = json?.results?.[0]?.response?.error || json?.results?.[0]?.error;
    if (firstResultError) {
      throw new Error(`Turso execute error: ${JSON.stringify(firstResultError)}`);
    }

    return json as TursoPipelineResponse;
  }

  async executeParams(sql: string, args: TursoArg[]): Promise<TursoPipelineResponse> {
    const token = requireEnv(this.tokenEnv);
    const body = {
      requests: [{ type: 'execute', stmt: { sql, args } } satisfies TursoPipelineRequest],
    };

    const res = await fetch(this.endpoint, {
      method: 'POST',
      headers: {
        Authorization: `Bearer ${token}`,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify(body),
    });

    if (!res.ok) {
      const text = await res.text().catch(() => '');
      throw new Error(`Turso HTTP error ${res.status} at ${this.endpoint}: ${text || res.statusText}`);
    }

    const json = (await res.json()) as any;
    const topLevelError = json?.error;
    if (topLevelError) {
      throw new Error(`Turso pipeline error: ${JSON.stringify(topLevelError)}`);
    }
    const firstResultError = json?.results?.[0]?.response?.error || json?.results?.[0]?.error;
    if (firstResultError) {
      throw new Error(`Turso execute error: ${JSON.stringify(firstResultError)}`);
    }

    return json as TursoPipelineResponse;
  }

  /**
   * Send all queries in a single HTTP request and return the full response.
   * Use this for read batches — each query's result is at response.results[i].
   */
  async executeBatch(sqls: string[]): Promise<TursoPipelineResponse> {
    const token = requireEnv(this.tokenEnv);
    const body = {
      requests: sqls.map((sql) => ({ type: 'execute', stmt: { sql } }) satisfies TursoPipelineRequest),
    };

    const res = await fetch(this.endpoint, {
      method: 'POST',
      headers: {
        Authorization: `Bearer ${token}`,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify(body),
    });

    if (!res.ok) {
      const text = await res.text().catch(() => '');
      throw new Error(`Turso HTTP error ${res.status} at ${this.endpoint}: ${text || res.statusText}`);
    }

    const json = (await res.json()) as any;

    const topLevelError = json?.error;
    if (topLevelError) {
      throw new Error(`Turso pipeline batch error: ${JSON.stringify(topLevelError)}`);
    }

    // Surface per-query errors early (check both common locations)
    for (let i = 0; i < sqls.length; i++) {
      const r = json.results?.[i];
      const err = r?.response?.error || r?.error;
      if (err) {
        throw new Error(`Turso batch query [${i}] error: ${JSON.stringify(err)}`);
      }
    }

    return json as TursoPipelineResponse;
  }

  async executeMany(sqlStatements: string[], batchSize = 25): Promise<void> {
    for (let i = 0; i < sqlStatements.length; i += batchSize) {
      const chunk = sqlStatements.slice(i, i + batchSize);
      // Use multiple pipeline requests to avoid relying on multi-statement support.
      const token = requireEnv(this.tokenEnv);
      const body = {
        requests: chunk.map((sql) => ({ type: 'execute', stmt: { sql } }) satisfies TursoPipelineRequest),
      };

      const res = await fetch(this.endpoint, {
        method: 'POST',
        headers: {
          Authorization: `Bearer ${token}`,
          'Content-Type': 'application/json',
        },
        body: JSON.stringify(body),
      });

      if (!res.ok) {
        const text = await res.text().catch(() => '');
        throw new Error(`Turso HTTP error ${res.status}: ${text || res.statusText}`);
      }

      const json = (await res.json()) as any;

      const topLevelError = json?.error;
      if (topLevelError) {
        throw new Error(`Turso pipeline executeMany error: ${JSON.stringify(topLevelError)}`);
      }

      const firstError = json.results?.find((r: any) => r?.response?.error || r?.error);
      const err = firstError?.response?.error || firstError?.error;
      if (err) {
        throw new Error(`Turso pipeline executeMany error: ${JSON.stringify(err)}`);
      }
    }
  }

  async executeManyParams(
    statements: Array<{ sql: string; args: TursoArg[] }>,
    batchSize = 25,
  ): Promise<void> {
    for (let i = 0; i < statements.length; i += batchSize) {
      const chunk = statements.slice(i, i + batchSize);
      const token = requireEnv(this.tokenEnv);
      const body = {
        requests: chunk.map(({ sql, args }) => ({ type: 'execute', stmt: { sql, args } }) satisfies TursoPipelineRequest),
      };

      const res = await fetch(this.endpoint, {
        method: 'POST',
        headers: {
          Authorization: `Bearer ${token}`,
          'Content-Type': 'application/json',
        },
        body: JSON.stringify(body),
      });

      if (!res.ok) {
        const text = await res.text().catch(() => '');
        throw new Error(`Turso HTTP error ${res.status}: ${text || res.statusText}`);
      }

      const json = (await res.json()) as any;
      const topLevelError = json?.error;
      if (topLevelError) {
        throw new Error(`Turso pipeline executeManyParams error: ${JSON.stringify(topLevelError)}`);
      }

      const firstError = json.results?.find((r: any) => r?.response?.error || r?.error);
      const err = firstError?.response?.error || firstError?.error;
      if (err) {
        throw new Error(`Turso pipeline executeManyParams error: ${JSON.stringify(err)}`);
      }
    }
  }
}
