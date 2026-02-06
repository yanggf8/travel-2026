import fs from 'node:fs';
import path from 'node:path';

type TursoPipelineRequest = {
  type: 'execute';
  stmt: {
    sql: string;
  };
};

type TursoPipelineResponse = {
  results?: Array<{
    response?: {
      result?: {
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

export class TursoPipelineClient {
  private endpoint: string;
  private tokenEnv: string;

  constructor(opts: TursoClientOptions = {}) {
    this.endpoint =
      opts.endpoint ||
      process.env.TURSO_HTTP_ENDPOINT ||
      'https://travel-2026-yanggf8.aws-ap-northeast-1.turso.io/v2/pipeline';
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
      throw new Error(`Turso HTTP error ${res.status}: ${text || res.statusText}`);
    }

    return (await res.json()) as TursoPipelineResponse;
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

      const json = (await res.json()) as TursoPipelineResponse;
      const firstError = json.results?.find((r) => r.response?.error)?.response?.error;
      if (firstError) {
        throw new Error(`Turso pipeline execute error: ${JSON.stringify(firstError)}`);
      }
    }
  }
}

