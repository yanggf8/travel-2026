export interface Env {
  TURSO_URL: string;
  TURSO_TOKEN: string;
}

interface TursoCell {
  type: string;
  value?: string;
}

interface TursoResult {
  cols: Array<{ name: string; decltype?: string }>;
  rows: TursoCell[][];
}

interface TursoPipelineResponse {
  results: Array<{
    response: {
      type: string;
      result?: TursoResult;
      error?: unknown;
    };
  }>;
}

function rowsToObjects(result: TursoResult): Record<string, string | null>[] {
  const colNames = result.cols.map((c) => c.name);
  return result.rows.map((row) => {
    const obj: Record<string, string | null> = {};
    colNames.forEach((name, i) => {
      obj[name] = row[i]?.value ?? null;
    });
    return obj;
  });
}

async function queryTurso(
  env: Env,
  sql: string
): Promise<Record<string, string | null>[]> {
  if (!env.TURSO_URL || !env.TURSO_TOKEN) {
    throw new Error('TURSO_URL and TURSO_TOKEN secrets are not configured. Run: wrangler secret put TURSO_URL && wrangler secret put TURSO_TOKEN');
  }
  const pipelineUrl = env.TURSO_URL.replace('libsql://', 'https://') + '/v2/pipeline';

  const body = {
    requests: [
      { type: 'execute', stmt: { sql } },
      { type: 'close' },
    ],
  };

  const res = await fetch(pipelineUrl, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${env.TURSO_TOKEN}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(body),
  });

  if (!res.ok) {
    const text = await res.text().catch(() => '');
    throw new Error(`Turso HTTP ${res.status}: ${text || res.statusText}`);
  }

  const json = (await res.json()) as TursoPipelineResponse;
  const first = json.results?.[0];

  if (first?.response?.error) {
    throw new Error(`Turso query error: ${JSON.stringify(first.response.error)}`);
  }

  const result = first?.response?.result;
  if (!result) return [];

  return rowsToObjects(result);
}

export interface PlanData {
  plan_json: string;
  state_json: string | null;
  updated_at: string;
}

function toDestSlug(s: string): string {
  return s.replace(/-/g, '_');
}

export async function getPlan(env: Env, planId: string): Promise<PlanData | null> {
  const escaped = planId.replace(/'/g, "''");
  const destSlug = toDestSlug(escaped);
  const rows = await queryTurso(
    env,
    `SELECT plan_json, state_json, updated_at FROM plans_current
     WHERE plan_id = '${escaped}'
        OR json_extract(plan_json, '$.active_destination') = '${destSlug}'
     ORDER BY (plan_id = '${escaped}') DESC
     LIMIT 1`
  );

  if (rows.length === 0) return null;
  return {
    plan_json: rows[0].plan_json!,
    state_json: rows[0].state_json ?? null,
    updated_at: rows[0].updated_at!,
  };
}

export interface BookingRow {
  booking_key: string;
  trip_id: string;
  destination: string;
  category: string;
  title: string;
  status: string;
  reference: string | null;
  book_by: string | null;
  price_amount: string | null;
  price_currency: string | null;
  payload_json: string | null;
}

export interface PlanSummary {
  slug: string;
  display_name: string;
  updated_at: string;
}

export async function listPlans(env: Env): Promise<PlanSummary[]> {
  const rows = await queryTurso(
    env,
    `SELECT plan_id,
      json_extract(plan_json, '$.active_destination') as active_dest,
      json_extract(plan_json, '$.destinations.' ||
        json_extract(plan_json, '$.active_destination') || '.display_name') as display_name,
      updated_at FROM plans_current ORDER BY updated_at DESC`
  );
  return rows.map((r) => {
    const dest = r.active_dest || r.plan_id!;
    return {
      slug: dest.replace(/_/g, '-'),
      display_name: r.display_name || dest.replace(/_/g, ' '),
      updated_at: r.updated_at!,
    };
  });
}

export async function getBookings(
  env: Env,
  destination: string
): Promise<BookingRow[]> {
  const escaped = destination.replace(/'/g, "''");
  const rows = await queryTurso(
    env,
    `SELECT booking_key, trip_id, destination, category, title, status, reference, book_by, price_amount, price_currency, payload_json FROM bookings_current WHERE destination = '${escaped}'`
  );

  return rows as unknown as BookingRow[];
}
