/**
 * StateManager regression tests
 *
 * These tests document expected behavior and catch regressions when:
 * - Activity search logic changes
 * - Time field updates break
 * - Event emission changes
 * - Legacy string activity upgrade breaks
 */

import { describe, it, expect, beforeEach } from 'vitest';
import * as path from 'node:path';
import { StateManager } from '../../src/state/state-manager';
import type { TravelPlanMinimal, TravelState } from '../../src/state/types';

// Minimal fixture with known structure
function createTestPlan(): TravelPlanMinimal {
  return {
    schema_version: '4.2.0',
    active_destination: 'tokyo_2026',
    process_1_date_anchor: {
      status: 'confirmed',
      start_date: '2026-02-13',
      end_date: '2026-02-17',
      num_days: 5,
      pax: 2,
    },
    destinations: {
      tokyo_2026: {
        slug: 'tokyo_2026',
        process_2_destination: { status: 'confirmed' },
        process_5_daily_itinerary: {
          status: 'researched',
          days: [
            {
              day_number: 1,
              date: '2026-02-13',
              morning: {
                focus: 'Arrival',
                activities: [
                  // Legacy string activity (should upgrade)
                  'Arrive at Narita',
                ],
              },
              afternoon: {
                focus: 'Check-in',
                activities: [
                  {
                    id: 'act-checkin',
                    title: 'Hotel check-in',
                    poi_ref: null,
                    duration_min: 30,
                    booking_required: false,
                    cost_estimate: null,
                    tags: [],
                    notes: null,
                  },
                ],
              },
              evening: {
                focus: 'Dinner',
                activities: [],
              },
            },
            {
              day_number: 5,
              date: '2026-02-17',
              morning: {
                focus: 'Checkout',
                activities: [
                  {
                    id: 'act-checkout',
                    title: 'Hotel checkout',
                    poi_ref: null,
                    duration_min: 30,
                    booking_required: false,
                    cost_estimate: null,
                    tags: [],
                    notes: null,
                  },
                ],
              },
              afternoon: {
                focus: 'Airport',
                activities: [
                  {
                    id: 'act-flight',
                    title: 'Flight departure',
                    poi_ref: null,
                    duration_min: 180,
                    booking_required: true,
                    booking_status: 'booked',
                    cost_estimate: null,
                    tags: ['transport'],
                    notes: null,
                  },
                ],
              },
              evening: { focus: null, activities: [] },
            },
          ],
        },
      },
    },
    cascade_rules: [],
    cascade_state: {
      destinations: {},
    },
  } as TravelPlanMinimal;
}

function createTestState(): TravelState {
  return {
    schema_version: '1.0.0',
    current_phase: 'planning',
    event_log: [],
    next_actions: [],
    dirty_flags: {},
  };
}

describe('StateManager.setActivityTime', () => {
  let sm: StateManager;

  beforeEach(() => {
    const plan = createTestPlan();
    const state = createTestState();
    sm = new StateManager({ plan, state, skipSave: true });
  });

  // ============================================================
  // REGRESSION: Activity search by ID (exact match)
  // ============================================================
  it('finds activity by exact ID', () => {
    sm.setActivityTime('tokyo_2026', 5, 'morning', 'act-checkout', {
      start_time: '11:00',
    });

    const day = sm.getDay('tokyo_2026', 5);
    const activity = (day?.morning as any).activities[0];
    expect(activity.start_time).toBe('11:00');
  });

  // ============================================================
  // REGRESSION: Activity search by title substring (case-insensitive)
  // ============================================================
  it('finds activity by title substring (case-insensitive)', () => {
    sm.setActivityTime('tokyo_2026', 5, 'morning', 'checkout', {
      start_time: '11:00',
    });

    const day = sm.getDay('tokyo_2026', 5);
    const activity = (day?.morning as any).activities[0];
    expect(activity.start_time).toBe('11:00');
  });

  it('finds activity by title substring with different case', () => {
    sm.setActivityTime('tokyo_2026', 5, 'morning', 'CHECKOUT', {
      start_time: '11:00',
    });

    const day = sm.getDay('tokyo_2026', 5);
    const activity = (day?.morning as any).activities[0];
    expect(activity.start_time).toBe('11:00');
  });

  // ============================================================
  // REGRESSION: Legacy string activity upgrade
  // ============================================================
  it('upgrades legacy string activity to object when setting time', () => {
    sm.setActivityTime('tokyo_2026', 1, 'morning', 'narita', {
      start_time: '18:00',
      is_fixed_time: true,
    });

    const day = sm.getDay('tokyo_2026', 1);
    const activity = (day?.morning as any).activities[0];

    // Should be upgraded from string to object
    expect(typeof activity).toBe('object');
    expect(activity.title).toContain('Arrive');
    expect(activity.start_time).toBe('18:00');
    expect(activity.is_fixed_time).toBe(true);
    // Should have generated ID
    expect(activity.id).toBeDefined();
  });

  // ============================================================
  // REGRESSION: Partial updates (only update provided fields)
  // ============================================================
  it('only updates provided fields, preserves others', () => {
    // First set start_time
    sm.setActivityTime('tokyo_2026', 5, 'morning', 'act-checkout', {
      start_time: '11:00',
    });

    // Then set end_time (should preserve start_time)
    sm.setActivityTime('tokyo_2026', 5, 'morning', 'act-checkout', {
      end_time: '11:30',
    });

    const day = sm.getDay('tokyo_2026', 5);
    const activity = (day?.morning as any).activities[0];

    expect(activity.start_time).toBe('11:00'); // Preserved
    expect(activity.end_time).toBe('11:30');   // Added
  });

  it('allows setting is_fixed_time to false explicitly', () => {
    // First set to true
    sm.setActivityTime('tokyo_2026', 5, 'morning', 'act-checkout', {
      is_fixed_time: true,
    });

    // Then set to false (must not be ignored)
    sm.setActivityTime('tokyo_2026', 5, 'morning', 'act-checkout', {
      is_fixed_time: false,
    });

    const day = sm.getDay('tokyo_2026', 5);
    const activity = (day?.morning as any).activities[0];
    expect(activity.is_fixed_time).toBe(false);
  });

  // ============================================================
  // REGRESSION: Event emission
  // ============================================================
  it('emits activity_time_updated event with from/to diff', () => {
    sm.setActivityTime('tokyo_2026', 5, 'morning', 'act-checkout', {
      start_time: '11:00',
      is_fixed_time: true,
    });

    const events = sm.getEventLog();
    const event = events.find(e => e.event === 'activity_time_updated');

    expect(event).toBeDefined();
    expect(event?.data?.day_number).toBe(5);
    expect(event?.data?.session).toBe('morning');
    expect(event?.data?.activity_id).toBe('act-checkout');
    expect(event?.data?.from?.start_time).toBeUndefined();
    expect(event?.data?.to?.start_time).toBe('11:00');
    expect(event?.data?.to?.is_fixed_time).toBe(true);
  });

  // ============================================================
  // REGRESSION: Error handling
  // ============================================================
  it('throws when day not found', () => {
    expect(() => {
      sm.setActivityTime('tokyo_2026', 99, 'morning', 'anything', {
        start_time: '10:00',
      });
    }).toThrow('Day 99 not found');
  });

  it('throws when activity not found', () => {
    expect(() => {
      sm.setActivityTime('tokyo_2026', 1, 'morning', 'nonexistent', {
        start_time: '10:00',
      });
    }).toThrow('Activity not found: "nonexistent"');
  });

  it('throws when session not found', () => {
    expect(() => {
      sm.setActivityTime('tokyo_2026', 1, 'evening', 'anything', {
        start_time: '10:00',
      });
    }).toThrow('Activity not found'); // evening.activities is empty
  });

  // ============================================================
  // REGRESSION: Timestamp tracking (touchItinerary)
  // ============================================================
  it('updates itinerary timestamp after change', () => {
    const plan = sm.getPlan();
    const p5Before = (plan.destinations['tokyo_2026'] as any).process_5_daily_itinerary;
    const beforeTs = p5Before?.updated_at;

    sm.setActivityTime('tokyo_2026', 5, 'morning', 'act-checkout', {
      start_time: '11:00',
    });

    const p5After = (sm.getPlan().destinations['tokyo_2026'] as any).process_5_daily_itinerary;
    expect(p5After.updated_at).toBeDefined();
    // Timestamp should be set (may be same or different depending on timing)
    expect(typeof p5After.updated_at).toBe('string');
  });
});

describe('StateManager.setSessionTimeRange', () => {
  let sm: StateManager;

  beforeEach(() => {
    const plan = createTestPlan();
    const state = createTestState();
    sm = new StateManager({ plan, state, skipSave: true });
  });

  it('sets time_range on session', () => {
    sm.setSessionTimeRange('tokyo_2026', 5, 'afternoon', '14:00', '19:55');

    const day = sm.getDay('tokyo_2026', 5);
    const timeRange = (day?.afternoon as any).time_range;

    expect(timeRange).toEqual({ start: '14:00', end: '19:55' });
  });

  it('emits session_time_range_updated event', () => {
    sm.setSessionTimeRange('tokyo_2026', 5, 'afternoon', '14:00', '19:55');

    const events = sm.getEventLog();
    const event = events.find(e => e.event === 'session_time_range_updated');

    expect(event).toBeDefined();
    expect(event?.data).toEqual({
      day_number: 5,
      session: 'afternoon',
      start: '14:00',
      end: '19:55',
    });
  });

  it('throws when day not found', () => {
    expect(() => {
      sm.setSessionTimeRange('tokyo_2026', 99, 'morning', '09:00', '12:00');
    }).toThrow('Day 99 not found');
  });
});

// ============================================================
// INTEGRATION: Activity search consistency
// ============================================================
describe('Activity search consistency across methods', () => {
  let sm: StateManager;

  beforeEach(() => {
    const plan = createTestPlan();
    const state = createTestState();
    sm = new StateManager({ plan, state, skipSave: true });
  });

  /**
   * REGRESSION: All activity methods must find the same activity
   * when given the same search term. If findActivityIndex is extracted
   * as a helper, this test ensures it's used consistently.
   */
  it('setActivityTime and setActivityBookingStatus find same activity', () => {
    // Use title substring
    sm.setActivityTime('tokyo_2026', 5, 'afternoon', 'flight', {
      start_time: '19:55',
    });

    sm.setActivityBookingStatus(
      'tokyo_2026', 5, 'afternoon', 'flight',
      'booked', 'ABC123'
    );

    const day = sm.getDay('tokyo_2026', 5);
    const activity = (day?.afternoon as any).activities[0];

    // Both should have modified the same activity
    expect(activity.start_time).toBe('19:55');
    expect(activity.booking_ref).toBe('ABC123');
  });
});

describe('StateManager.derivePlanId', () => {
  it('is stable for relative and absolute custom paths', () => {
    const rel = 'data/custom/travel-plan.json';
    const abs = path.resolve(rel);

    const relId = StateManager.derivePlanId(rel);
    const absId = StateManager.derivePlanId(abs);

    expect(relId).toBe(absId);
    expect(relId.startsWith('path:')).toBe(true);
  });

  it('keeps default and trip mapping semantics', () => {
    expect(StateManager.derivePlanId('data/travel-plan.json')).toBe('default');
    expect(StateManager.derivePlanId(path.resolve('data/travel-plan.json'))).toBe('default');

    expect(StateManager.derivePlanId('data/trips/osaka-2026/travel-plan.json')).toBe('osaka-2026');
    expect(StateManager.derivePlanId(path.resolve('data/trips/osaka-2026/travel-plan.json'))).toBe('osaka-2026');
  });
});
