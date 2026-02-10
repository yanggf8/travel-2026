export interface Env {
  TURSO_URL: string;
  TURSO_TOKEN: string;
  DEFAULT_PLAN_ID: string;
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

export async function getPlan(env: Env, planId: string): Promise<PlanData | null> {
  const escaped = planId.replace(/'/g, "''");
  const rows = await queryTurso(
    env,
    `SELECT plan_json, state_json, updated_at FROM plans_current WHERE plan_id = '${escaped}' LIMIT 1`
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
  label: string;
  status: string;
  details_json: string | null;
}

export async function getBookings(
  env: Env,
  destination: string
): Promise<BookingRow[]> {
  const escaped = destination.replace(/'/g, "''");
  const rows = await queryTurso(
    env,
    `SELECT booking_key, trip_id, destination, category, label, status, details_json FROM bookings_current WHERE destination = '${escaped}'`
  );

  return rows as unknown as BookingRow[];
}
