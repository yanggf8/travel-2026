import { describe, expect, it } from 'vitest';
import { resolvePlanFromSummaries, type PlanSummary } from '../../src/cli/shared/plan-resolver';

const pastTokyo: PlanSummary = {
  planId: 'tokyo-2026',
  activeDestination: 'tokyo_2026',
  updatedAt: '2026-02-17 19:24:02',
  anchors: [{ destination: 'tokyo_2026', startDate: '2026-02-13', endDate: '2026-02-17', days: 5 }],
};

const pastKyoto: PlanSummary = {
  planId: 'kyoto-2026',
  activeDestination: 'kyoto_2026',
  updatedAt: '2026-02-28 15:51:22',
  anchors: [{ destination: 'kyoto_2026', startDate: '2026-02-24', endDate: '2026-02-28', days: 5 }],
};

const junePlan: PlanSummary = {
  planId: 'japan-june-2026',
  activeDestination: 'fukuoka_2026',
  updatedAt: '2026-05-22 10:00:00',
  anchors: [{ destination: 'fukuoka_2026', startDate: '2026-06-18', endDate: '2026-06-25', days: 8 }],
};

const juneCandidateWindow: PlanSummary = {
  planId: 'japan-june-window',
  activeDestination: 'undecided_japan_2026',
  updatedAt: '2026-05-22 12:00:00',
  anchors: [],
  planningWindow: { status: 'researching', startDate: '2026-06-18', endDate: '2026-06-25' },
};

describe('CLI plan resolver', () => {
  it('selects the plan whose date anchor contains the requested travel date', () => {
    const resolved = resolvePlanFromSummaries(
      { date: '2026-06-20', today: '2026-05-22' },
      [pastTokyo, pastKyoto, junePlan],
    );

    expect(resolved).toMatchObject({ planId: 'japan-june-2026', source: 'date' });
  });

  it('selects a single upcoming plan when no date is requested', () => {
    const resolved = resolvePlanFromSummaries(
      { today: '2026-05-22' },
      [pastTokyo, pastKyoto, junePlan],
    );

    expect(resolved).toMatchObject({ planId: 'japan-june-2026', source: 'upcoming' });
  });

  it('can resolve a candidate planning window before dates are committed', () => {
    const resolved = resolvePlanFromSummaries(
      { rangeStart: '2026-06-18', rangeEnd: '2026-06-25', today: '2026-05-22' },
      [pastTokyo, pastKyoto, juneCandidateWindow],
    );

    expect(resolved).toMatchObject({ planId: 'japan-june-window', source: 'date' });
  });

  it('does not silently choose when multiple future plans overlap the same date', () => {
    const otherJunePlan: PlanSummary = {
      ...junePlan,
      planId: 'osaka-june-2026',
      activeDestination: 'osaka_2026',
      anchors: [{ destination: 'osaka_2026', startDate: '2026-06-20', endDate: '2026-06-24', days: 5 }],
    };

    expect(() => resolvePlanFromSummaries(
      { date: '2026-06-21', today: '2026-05-22' },
      [junePlan, otherJunePlan],
    )).toThrow(/Multiple travel plans contain 2026-06-21/);
  });

  it('falls back to latest updated only when all known plans are historical', () => {
    const resolved = resolvePlanFromSummaries(
      { today: '2026-05-22' },
      [pastTokyo, pastKyoto],
    );

    expect(resolved).toMatchObject({ planId: 'kyoto-2026', source: 'latest' });
  });
});
