import { rowsToObjects, sqlText } from '../../state/sql-helpers';
import { StateManager } from '../../state/state-manager';

function requirePipeline(): { TursoPipelineClient: new (opts?: any) => any } {
  const path = require('node:path');
  return require(path.resolve(__dirname, '..', '..', '..', 'scripts', 'turso-pipeline'));
}

export interface PlanAnchor {
  destination: string;
  startDate: string;
  endDate: string;
  days: number;
}

export interface PlanSummary {
  planId: string;
  activeDestination: string;
  updatedAt: string;
  anchors: PlanAnchor[];
  planningWindow?: {
    status: string;
    startDate?: string;
    endDate?: string;
    durationDays?: number;
  };
}

export interface ResolvedPlan {
  planId: string;
  source: 'explicit' | 'env' | 'plan-path' | 'date' | 'active' | 'upcoming' | 'latest';
  note?: string;
}

interface ResolveInput {
  explicitPlanId?: string;
  envPlanId?: string;
  planPath?: string;
  date?: string;
  rangeStart?: string;
  rangeEnd?: string;
  today?: string;
}

export function isIsoDate(value: string | undefined): value is string {
  return Boolean(value && /^\d{4}-\d{2}-\d{2}$/.test(value));
}

export function todayIso(): string {
  const now = new Date();
  const year = now.getFullYear();
  const month = String(now.getMonth() + 1).padStart(2, '0');
  const day = String(now.getDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
}

export async function listPlans(): Promise<PlanSummary[]> {
  const { TursoPipelineClient } = requirePipeline();
  const client = new TursoPipelineClient();
  const response = await client.execute(`
    SELECT
      pm.plan_id,
      pm.active_destination,
      pm.updated_at,
      da.destination,
      da.start_date,
      da.end_date,
      da.days,
      p1.status AS p1_status,
      p1.set_out_date AS p1_set_out_date,
      p1.return_date AS p1_return_date,
      p1.duration_days AS p1_duration_days,
      p1.flexibility_json AS p1_flexibility_json
    FROM plan_metadata pm
    LEFT JOIN date_anchors da ON da.plan_id = pm.plan_id
    LEFT JOIN plan_root_date_anchor p1 ON p1.plan_id = pm.plan_id
    ORDER BY pm.updated_at DESC, da.start_date ASC
  `);
  return groupPlanRows(rowsToObjects(response));
}

export function groupPlanRows(rows: Record<string, any>[]): PlanSummary[] {
  const byId = new Map<string, PlanSummary>();
  for (const row of rows) {
    const planId = String(row.plan_id || '');
    if (!planId) continue;
    let summary = byId.get(planId);
    if (!summary) {
      summary = {
        planId,
        activeDestination: String(row.active_destination || ''),
        updatedAt: String(row.updated_at || ''),
        anchors: [],
      };
      byId.set(planId, summary);
    }
    if (!summary.planningWindow && row.p1_status) {
      const flexibility = parseJson(row.p1_flexibility_json);
      const windowStart = stringValue(
        flexibility?.window_start
        ?? flexibility?.travel_start
        ?? flexibility?.start_date
        ?? row.p1_set_out_date,
      );
      const windowEnd = stringValue(
        flexibility?.window_end
        ?? flexibility?.travel_end
        ?? flexibility?.end_date
        ?? row.p1_return_date,
      );
      summary.planningWindow = {
        status: String(row.p1_status),
        startDate: windowStart,
        endDate: windowEnd,
        durationDays: row.p1_duration_days ? Number(row.p1_duration_days) : undefined,
      };
    }
    if (row.destination && row.start_date && row.end_date) {
      summary.anchors.push({
        destination: String(row.destination),
        startDate: String(row.start_date),
        endDate: String(row.end_date),
        days: Number(row.days || 0),
      });
    }
  }
  return Array.from(byId.values());
}

export function resolvePlanFromSummaries(input: ResolveInput, plans: PlanSummary[]): ResolvedPlan {
  if (input.explicitPlanId) return { planId: input.explicitPlanId, source: 'explicit' };
  if (input.envPlanId) return { planId: input.envPlanId, source: 'env' };
  if (input.planPath) return { planId: StateManager.derivePlanId(input.planPath), source: 'plan-path' };

  const today = input.today || todayIso();
  const date = normalizeDateInput(input.date);
  const rangeStart = normalizeDateInput(input.rangeStart);
  const rangeEnd = normalizeDateInput(input.rangeEnd);

  if (date) {
    return pickSinglePlan(
      plansWithDate(plans, date, date),
      `No travel plan contains ${date}.`,
      `Multiple travel plans contain ${date}.`,
      'date',
      plans,
    );
  }

  if (rangeStart || rangeEnd) {
    const start = rangeStart || rangeEnd!;
    const end = rangeEnd || rangeStart!;
    return pickSinglePlan(
      plansWithDate(plans, start, end),
      `No travel plan overlaps ${start} to ${end}.`,
      `Multiple travel plans overlap ${start} to ${end}.`,
      'date',
      plans,
    );
  }

  const active = plansWithDate(plans, today, today);
  if (active.length === 1) return { planId: active[0].planId, source: 'active' };
  if (active.length > 1) {
    throw new Error(formatAmbiguousPlanError(`Multiple travel plans are active on ${today}.`, active));
  }

  const upcoming = plans.filter((plan) => {
    if (plan.anchors.some((anchor) => anchor.endDate >= today)) return true;
    const window = plan.planningWindow;
    return Boolean(window?.endDate && window.endDate >= today);
  });
  if (upcoming.length === 1) return { planId: upcoming[0].planId, source: 'upcoming' };
  if (upcoming.length > 1) {
    throw new Error(formatAmbiguousPlanError('Multiple upcoming travel plans exist.', upcoming));
  }

  if (plans.length === 1) {
    return { planId: plans[0].planId, source: 'latest', note: 'Only one plan exists.' };
  }
  if (plans.length > 1) {
    const latest = [...plans].sort((a, b) => b.updatedAt.localeCompare(a.updatedAt))[0];
    return {
      planId: latest.planId,
      source: 'latest',
      note: 'No active/upcoming plan exists; using most recently updated plan.',
    };
  }

  throw new Error('No travel plans found in DB. Create or seed a plan, then retry.');
}

export function formatPlanList(plans: PlanSummary[], today = todayIso()): string {
  if (plans.length === 0) return 'No travel plans found.';
  const rows = plans.map((plan) => {
    const anchors = plan.anchors.length > 0
      ? plan.anchors.map((a) => `${a.destination}: ${a.startDate}..${a.endDate}`).join('; ')
      : '(no date anchor)';
    const window = plan.planningWindow?.startDate || plan.planningWindow?.endDate
      ? ` window=${plan.planningWindow.startDate || '?'}..${plan.planningWindow.endDate || '?'}`
      : '';
    const status = planStatus(plan, today);
    return `${plan.planId.padEnd(16)} ${status.padEnd(9)} ${plan.activeDestination.padEnd(16)} ${anchors}${window}`;
  });
  return [
    'PLAN_ID          STATUS    ACTIVE_DEST      DATE_ANCHORS',
    '---------------  --------  ---------------  ------------------------------',
    ...rows,
  ].join('\n');
}

function normalizeDateInput(value: string | undefined): string | undefined {
  if (!value) return undefined;
  if (!isIsoDate(value)) throw new Error(`Invalid date "${value}". Expected YYYY-MM-DD.`);
  return value;
}

function plansWithDate(plans: PlanSummary[], start: string, end: string): PlanSummary[] {
  return plans.filter((plan) => {
    if (plan.anchors.some((anchor) => anchor.startDate <= end && anchor.endDate >= start)) return true;
    const window = plan.planningWindow;
    return Boolean(window?.startDate && window?.endDate && window.startDate <= end && window.endDate >= start);
  });
}

function pickSinglePlan(
  matches: PlanSummary[],
  noneMessage: string,
  manyMessage: string,
  source: ResolvedPlan['source'],
  allPlans: PlanSummary[],
): ResolvedPlan {
  if (matches.length === 1) return { planId: matches[0].planId, source };
  if (matches.length === 0) throw new Error(`${noneMessage}\n\n${formatPlanList(allPlans)}`);
  throw new Error(formatAmbiguousPlanError(manyMessage, matches));
}

function formatAmbiguousPlanError(message: string, plans: PlanSummary[]): string {
  return `${message}\nUse --plan-id <id> or narrow with --travel-date/--travel-start/--travel-end.\n\n${formatPlanList(plans)}`;
}

function planStatus(plan: PlanSummary, today: string): string {
  if (plan.anchors.some((a) => a.startDate <= today && a.endDate >= today)) return 'active';
  if (plan.anchors.some((a) => a.endDate >= today)) return 'upcoming';
  const window = plan.planningWindow;
  if (window?.startDate && window?.endDate && window.startDate <= today && window.endDate >= today) return 'planning';
  if (window?.endDate && window.endDate >= today) return 'candidate';
  return 'past';
}

function parseJson(value: unknown): any {
  if (typeof value !== 'string' || !value) return null;
  try {
    return JSON.parse(value);
  } catch {
    return null;
  }
}

function stringValue(value: unknown): string | undefined {
  return typeof value === 'string' && value ? value : undefined;
}

export async function planExists(planId: string): Promise<boolean> {
  const { TursoPipelineClient } = requirePipeline();
  const client = new TursoPipelineClient();
  const response = await client.execute(`SELECT plan_id FROM plan_metadata WHERE plan_id = ${sqlText(planId)} LIMIT 1`);
  return rowsToObjects(response).length > 0;
}
